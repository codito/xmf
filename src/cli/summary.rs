use super::ui;
use crate::core::config::{Investment, Portfolio};
use crate::core::{
    CurrencyRateProvider, PriceProvider, PriceResult, analytics, analytics::PortfolioValue,
};
use anyhow::Result;
use chrono::NaiveDate;
use comfy_table::Cell;
use console::style;
use futures::future::join_all;
use std::collections::HashMap;

fn to_superscript(n: usize) -> String {
    n.to_string()
        .chars()
        .map(|c| match c {
            '0' => '\u{2070}',
            '1' => '\u{00B9}',
            '2' => '\u{00B2}',
            '3' => '\u{00B3}',
            '4' => '\u{2074}',
            '5' => '\u{2075}',
            '6' => '\u{2076}',
            '7' => '\u{2077}',
            '8' => '\u{2078}',
            '9' => '\u{2079}',
            _ => c,
        })
        .collect()
}

impl PortfolioValue {
    pub fn display_as_table(&self) -> String {
        let target_currency = &self.target_currency;

        // Find the latest as_of_date across all investments
        let all_dates: Vec<NaiveDate> = self
            .investments
            .iter()
            .filter_map(|i| i.as_of_date)
            .collect();
        let latest_date = all_dates.iter().max().copied();

        // Collect stale dates (< latest_date), deduplicate, sort newest-first
        let stale_dates: Vec<NaiveDate> = if let Some(latest) = latest_date {
            let mut dates: Vec<NaiveDate> =
                all_dates.iter().filter(|&&d| d < latest).copied().collect();
            dates.sort();
            dates.dedup();
            dates.reverse();
            dates
        } else {
            Vec::new()
        };

        let superscript_map: HashMap<NaiveDate, String> = stale_dates
            .iter()
            .enumerate()
            .map(|(i, &date)| (date, to_superscript(i + 1)))
            .collect();

        let mut table = ui::new_styled_table();

        table.set_header(vec![
            ui::header_cell("Investment"),
            ui::header_cell("Units"),
            ui::header_cell("Price"),
            ui::header_cell(&format!("Value ({target_currency})")),
            ui::header_cell("Weight"),
        ]);

        for investment in &self.investments {
            let currency = investment
                .value_currency
                .as_deref()
                .unwrap_or("N/A")
                .to_string();

            let name_display = if let Some(name) = &investment.short_name {
                name.clone()
            } else {
                investment.identifier.clone()
            };

            let units = ui::format_optional_cell(investment.units, |u| format!("{u:.2}"));

            let sup = investment.as_of_date.and_then(|d| superscript_map.get(&d));
            let has_any_superscript = !stale_dates.is_empty();
            let current_price = ui::format_cell(investment.price, |opt| match opt {
                Some(p) => {
                    let mut s = format!("{p:.2}{currency}");
                    if let Some(sup) = sup {
                        s.push_str(sup);
                    } else if has_any_superscript {
                        s.push(' ');
                    }
                    s
                }
                None => {
                    let mut s = "N/A".to_string();
                    if has_any_superscript {
                        s.push(' ');
                    }
                    s
                }
            });
            let converted_value =
                ui::format_optional_cell(investment.converted_value, |v| format!("{v:.2}"));
            let weight_pct = ui::format_optional_cell(investment.weight, |w| format!("{w:.2}%"));

            table.add_row(vec![
                Cell::new(name_display),
                units,
                current_price,
                converted_value,
                weight_pct,
            ]);
        }

        let total_style_type = if self.total_converted_value.is_some() {
            ui::StyleType::TotalValue
        } else {
            ui::StyleType::Error
        };
        let total_converted_value = self
            .total_converted_value
            .map_or("N/A".to_string(), |v| format!("{v:.2}"));

        // Portfolio name at top
        let mut output = format!(
            "Portfolio: {}\n\n",
            ui::style_text(&self.name, ui::StyleType::Title)
        );

        // Table in the middle
        output.push_str(&table.to_string());

        // Total value at bottom
        output.push_str(&format!(
            "\n\nTotal Value ({}): {}",
            ui::style_text(target_currency, ui::StyleType::TotalLabel),
            ui::style_text(&total_converted_value, total_style_type)
        ));

        if let Some(latest) = latest_date {
            if !stale_dates.is_empty() {
                let markers: String = stale_dates
                    .iter()
                    .map(|d| {
                        let sup = &superscript_map[d];
                        format!(" {}{}", sup, d.format("%Y-%m-%d"))
                    })
                    .collect::<Vec<_>>()
                    .join("");
                output.push_str(&format!(
                    "\nData as of:{}; others {}",
                    markers,
                    latest.format("%Y-%m-%d")
                ));
            } else {
                output.push_str(&format!("\nData as of: {}", latest.format("%Y-%m-%d")));
            }
        }

        output
    }
}

pub async fn run(
    portfolios: &[Portfolio],
    symbol_provider: &(dyn PriceProvider + Send + Sync),
    isin_provider: &(dyn PriceProvider + Send + Sync),
    currency_provider: &(dyn CurrencyRateProvider + Send + Sync),
    target_currency: &str,
) -> Result<()> {
    let mut investments_to_fetch = HashMap::new();
    for portfolio in portfolios {
        for investment in &portfolio.investments {
            match investment {
                Investment::Stock(s) => {
                    investments_to_fetch.insert(s.symbol.clone(), symbol_provider);
                }
                Investment::MutualFund(mf) => {
                    investments_to_fetch.insert(mf.isin.clone(), isin_provider);
                }
                Investment::FixedDeposit(_) => {}
            }
        }
    }

    let pb = ui::new_progress_bar(investments_to_fetch.len() as u64, true);
    pb.set_message("Fetching prices...");

    let price_futures = investments_to_fetch.iter().map(|(id, provider)| {
        let pb_clone = pb.clone();
        async move {
            let res = provider.fetch_price(id).await;
            pb_clone.inc(1);
            (id.clone(), res)
        }
    });

    let price_results: HashMap<String, Result<PriceResult>> =
        join_all(price_futures).await.into_iter().collect();
    pb.finish_and_clear();

    // Step 1: Process portfolios to calculate holdings
    let total_investments: u64 = portfolios
        .iter()
        .map(|p| p.investments.len())
        .sum::<usize>() as u64;
    let pb = ui::new_progress_bar(total_investments, true);
    pb.set_message("Processing investments...");

    let holdings_futures = portfolios.iter().map(|portfolio| {
        let pb_clone = pb.clone();
        let price_results = &price_results;
        async move {
            analytics::calculate_portfolio_value(
                portfolio,
                price_results,
                currency_provider,
                target_currency,
                &|| pb_clone.inc(1),
            )
            .await
        }
    });

    let summaries = join_all(holdings_futures).await;
    pb.finish_and_clear();

    // Step 2: Calculate grand total and display summaries
    let mut grand_total = 0.0;
    let mut all_portfolios_valid = true;

    for sum in &summaries {
        if let Some(value) = sum.total_converted_value {
            grand_total += value;
        } else {
            all_portfolios_valid = false;
        }
    }

    let num_summaries = summaries.len();
    for (i, sum) in summaries.into_iter().enumerate() {
        println!("{}", sum.display_as_table());
        if i < num_summaries - 1 {
            ui::print_separator();
        }
    }

    if all_portfolios_valid && num_summaries > 1 {
        let term_width = console::Term::stdout()
            .size_checked()
            .map(|(_, w)| w as usize)
            .unwrap_or(80);
        println!("\n{}", "=".repeat(term_width));
        let total_str = format!("Grand Total ({target_currency}): {grand_total:.2}");
        let styled_total = style(&total_str).bold().green();
        println!("{styled_total:>term_width$}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::to_superscript;

    #[test]
    fn test_to_superscript_digits() {
        assert_eq!(to_superscript(0), "⁰");
        assert_eq!(to_superscript(1), "¹");
        assert_eq!(to_superscript(2), "²");
        assert_eq!(to_superscript(3), "³");
        assert_eq!(to_superscript(4), "⁴");
        assert_eq!(to_superscript(5), "⁵");
        assert_eq!(to_superscript(6), "⁶");
        assert_eq!(to_superscript(7), "⁷");
        assert_eq!(to_superscript(8), "⁸");
        assert_eq!(to_superscript(9), "⁹");
    }

    #[test]
    fn test_to_superscript_multi_digit() {
        assert_eq!(to_superscript(10), "¹⁰");
        assert_eq!(to_superscript(42), "⁴²");
        assert_eq!(to_superscript(100), "¹⁰⁰");
        assert_eq!(to_superscript(2026), "²⁰²⁶");
    }
}

use super::ui;
use crate::core::config::{Investment, Portfolio};
use crate::core::{analytics, CurrencyRateProvider, PriceProvider, PriceResult};
use anyhow::Result;
use comfy_table::Cell;
use console::style;
use futures::future::join_all;
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct InvestmentSummary {
    pub symbol: String,
    pub short_name: Option<String>,
    pub units: Option<f64>,
    pub current_price: Option<f64>,
    pub current_value: Option<f64>,
    pub currency: Option<String>,
    pub converted_value: Option<f64>,
    pub weight_pct: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct PortfolioSummary {
    pub name: String,
    pub total_value: Option<f64>,
    pub converted_value: Option<f64>,
    pub currency: Option<String>,
    pub investments: Vec<InvestmentSummary>,
}

impl PortfolioSummary {
    pub fn display_as_table(&self) -> String {
        let target_currency = self.currency.as_deref().unwrap_or("N/A");

        let mut table = ui::new_styled_table();

        table.set_header(vec![
            ui::header_cell("Symbol"),
            ui::header_cell("Units"),
            ui::header_cell("Price"),
            ui::header_cell(&format!("Value ({target_currency})")),
            ui::header_cell("Weight (%)"),
        ]);

        for investment in &self.investments {
            let currency = investment.currency.as_deref().unwrap_or("N/A").to_string();

            let symbol_cell_content = if let Some(name) = &investment.short_name {
                name.clone()
            } else {
                investment.symbol.clone()
            };

            let units = ui::format_optional_cell(investment.units, |u| format!("{u:.2}"));
            let current_price =
                ui::format_optional_cell(investment.current_price, |p| format!("{p:.2}{currency}"));
            let converted_value =
                ui::format_optional_cell(investment.converted_value, |v| format!("{v:.2}"));
            let weight_pct =
                ui::format_optional_cell(investment.weight_pct, |w| format!("{w:.2}%"));

            table.add_row(vec![
                Cell::new(symbol_cell_content),
                units,
                current_price,
                converted_value,
                weight_pct,
            ]);
        }

        let total_style_type = if self.converted_value.is_some() {
            ui::StyleType::TotalValue
        } else {
            ui::StyleType::Error
        };
        let total_converted_value = self
            .converted_value
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
            analytics::calculate_portfolio_holdings(
                portfolio,
                price_results,
                currency_provider,
                target_currency,
                &|| pb_clone.inc(1),
            )
            .await
        }
    });

    let holdings_results = join_all(holdings_futures).await;
    pb.finish_and_clear();

    // Map analytics results to display models
    let summaries: Vec<PortfolioSummary> = holdings_results
        .into_iter()
        .map(|holdings| {
            let investments = holdings
                .investments
                .into_iter()
                .map(|h| InvestmentSummary {
                    symbol: h.identifier,
                    short_name: h.short_name,
                    units: h.units,
                    current_price: h.price,
                    current_value: h.value,
                    currency: h.value_currency,
                    converted_value: h.converted_value,
                    weight_pct: h.weight,
                    error: h.error,
                })
                .collect();

            PortfolioSummary {
                name: holdings.name,
                total_value: None, // Not calculated in analytics, not used for display
                converted_value: holdings.total_converted_value,
                currency: Some(holdings.target_currency),
                investments,
            }
        })
        .collect();

    // Step 2: Calculate grand total and display summaries
    let mut grand_total = 0.0;
    let mut all_portfolios_valid = true;

    for sum in &summaries {
        if let Some(value) = sum.converted_value {
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


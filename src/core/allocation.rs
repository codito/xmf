use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetCategory {
    Equity,
    Debt,
    Hybrid,
    Other,
}

impl From<&str> for AssetCategory {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "equity" => AssetCategory::Equity,
            "debt" | "income" | "fixed income" => AssetCategory::Debt,
            "hybrid" | "balanced" | "dynamic" => AssetCategory::Hybrid,
            _ => AssetCategory::Other,
        }
    }
}

impl AssetCategory {
    /// Returns display name and emoji for the category
    pub fn display_info(&self) -> (&'static str, &'static str) {
        match self {
            AssetCategory::Equity => ("Equity", "📈"),
            AssetCategory::Debt => ("Debt", "📉"),
            AssetCategory::Hybrid => ("Hybrid", "📊"),
            AssetCategory::Other => ("Other", "❓"),
        }
    }
}

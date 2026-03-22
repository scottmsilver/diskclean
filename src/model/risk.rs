use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskLevel {
    Safe,
    ReviewFirst,
    Caution,
}

impl RiskLevel {
    pub fn style(&self) -> Style {
        match self {
            Self::Safe => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            Self::ReviewFirst => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            Self::Caution => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Safe => "SAFE TO DELETE",
            Self::ReviewFirst => "REVIEW FIRST",
            Self::Caution => "USE CAUTION",
        }
    }

    pub fn short(&self) -> &str {
        match self {
            Self::Safe => "S",
            Self::ReviewFirst => "R",
            Self::Caution => "C",
        }
    }
}

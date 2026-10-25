//! Display units. Everything inside CosmicCAM is millimetres; the user's units preference
//! only changes what is shown and what dialogs accept. The command API always takes and
//! returns millimetres, so a script means the same thing whatever the preference.

/// Units lengths, positions and feeds are shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayUnits {
    Millimetres,
    #[default]
    Inches,
}

impl DisplayUnits {
    pub const IDS: &'static [&'static str] = &["in", "mm"];

    pub fn id(self) -> &'static str {
        match self {
            Self::Millimetres => "mm",
            Self::Inches => "in",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "mm" => Some(Self::Millimetres),
            "in" => Some(Self::Inches),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Millimetres => "Millimetres",
            Self::Inches => "Inches",
        }
    }

    /// Millimetres per display unit.
    pub fn mm_per_unit(self) -> f64 {
        match self {
            Self::Millimetres => 1.0,
            Self::Inches => 25.4,
        }
    }

    /// The length symbol: "mm" or "in".
    pub fn length(self) -> &'static str {
        self.id()
    }

    /// The feed symbol: "mm/min" or "in/min".
    pub fn rate(self) -> &'static str {
        match self {
            Self::Millimetres => "mm/min",
            Self::Inches => "in/min",
        }
    }

    /// Decimal places shown for lengths.
    pub fn places(self) -> usize {
        match self {
            Self::Millimetres => 3,
            Self::Inches => 4,
        }
    }

    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm_per_unit()
    }

    pub fn to_mm(self, v: f64) -> f64 {
        v * self.mm_per_unit()
    }

    /// "12.700 mm" / "0.5000 in".
    pub fn fmt_len(self, mm: f64) -> String {
        format!("{:.*} {}", self.places(), self.from_mm(mm), self.length())
    }

    /// A number in display units without the symbol, trailing zeros trimmed: "0.5", "12.7".
    pub fn plain(self, mm: f64) -> String {
        trim(format!("{:.*}", self.places(), self.from_mm(mm)))
    }

    /// "1200 mm/min" / "47.244 in/min".
    pub fn fmt_rate(self, mm_per_min: f64) -> String {
        format!("{} {}", self.plain_rate(mm_per_min), self.rate())
    }

    /// A feed in display units without the symbol: "1200", "47.244".
    pub fn plain_rate(self, mm_per_min: f64) -> String {
        let places = if self == Self::Millimetres { 1 } else { 3 };
        trim(format!("{:.*}", places, self.from_mm(mm_per_min)))
    }

    /// "1.000, 2.000, 3.000 mm" for a point.
    pub fn fmt_point(self, p: [f32; 3]) -> String {
        let c = |v: f32| format!("{:.*}", self.places(), self.from_mm(f64::from(v)));
        format!("{}, {}, {} {}", c(p[0]), c(p[1]), c(p[2]), self.length())
    }

    /// "100.0 × 60.0 × 42.0 mm" for a size.
    pub fn fmt_size(self, s: [f32; 3]) -> String {
        let places = if self == Self::Millimetres { 2 } else { 3 };
        let c = |v: f32| format!("{:.*}", places, self.from_mm(f64::from(v)));
        format!("{} × {} × {} {}", c(s[0]), c(s[1]), c(s[2]), self.length())
    }

    /// "1657.577 mm²" / "2.5693 in²".
    pub fn fmt_area(self, mm2: f64) -> String {
        format!("{:.*} {}²", self.places(), mm2 / (self.mm_per_unit() * self.mm_per_unit()), self.length())
    }
}

fn trim(s: String) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_converts() {
        let i = DisplayUnits::Inches;
        assert_eq!(i.fmt_len(25.4), "1.0000 in");
        assert_eq!(i.plain(12.7), "0.5");
        assert_eq!(i.fmt_rate(2540.0), "100 in/min");
        assert!((i.to_mm(0.25) - 6.35).abs() < 1e-12);
        let m = DisplayUnits::Millimetres;
        assert_eq!(m.fmt_len(6.0), "6.000 mm");
        assert_eq!(m.plain(6.0), "6");
        assert_eq!(m.fmt_rate(1200.0), "1200 mm/min");
        assert_eq!(m.fmt_size([100.0, 60.0, 42.0]), "100.00 × 60.00 × 42.00 mm");
        assert_eq!(DisplayUnits::default(), DisplayUnits::Inches);
    }
}

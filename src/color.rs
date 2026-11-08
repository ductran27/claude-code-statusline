//! Muted 256-color palette. No bright codes; bold only for labels.

pub const RESET: &str = "\x1b[0m";
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const GREEN: &str = "\x1b[38;5;108m"; // sage
pub const YELLOW: &str = "\x1b[38;5;179m"; // gold
pub const ORANGE: &str = "\x1b[38;5;173m"; // terra cotta
pub const RED: &str = "\x1b[38;5;167m"; // brick
pub const SLATE: &str = "\x1b[38;5;103m";
pub const MAUVE: &str = "\x1b[38;5;139m";
pub const ROSE: &str = "\x1b[38;5;181m";
pub const GRAY: &str = "\x1b[38;5;245m";
pub const WHITE_DIM: &str = "\x1b[38;5;250m";

/// Segment separator, no surrounding spaces.
pub const SEP: &str = "\x1b[2m\x1b[38;5;245m│\x1b[0m";

/// Green, yellow, orange, red as `pct` crosses the three thresholds.
pub fn by_pct(pct: i64, t: (i64, i64, i64)) -> &'static str {
    if pct <= t.0 {
        GREEN
    } else if pct <= t.1 {
        YELLOW
    } else if pct <= t.2 {
        ORANGE
    } else {
        RED
    }
}

pub const TOKEN_THRESH: (i64, i64, i64) = (30, 60, 80);
pub const RATE_THRESH: (i64, i64, i64) = (50, 75, 90);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_inclusive() {
        assert_eq!(by_pct(50, RATE_THRESH), GREEN);
        assert_eq!(by_pct(51, RATE_THRESH), YELLOW);
        assert_eq!(by_pct(75, RATE_THRESH), YELLOW);
        assert_eq!(by_pct(90, RATE_THRESH), ORANGE);
        assert_eq!(by_pct(91, RATE_THRESH), RED);
        assert_eq!(by_pct(0, TOKEN_THRESH), GREEN);
        assert_eq!(by_pct(81, TOKEN_THRESH), RED);
    }
}

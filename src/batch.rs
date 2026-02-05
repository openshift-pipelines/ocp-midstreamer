//! Batch execution module for date range iteration.
//!
//! Provides date range parsing, validation, and progress tracking for
//! running historical builds across multiple dates.

use chrono::{Duration, NaiveDate};

/// A date range for batch historical runs.
#[derive(Debug, Clone)]
pub struct DateRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl DateRange {
    /// Returns the number of days in this range (inclusive).
    pub fn day_count(&self) -> i64 {
        (self.end - self.start).num_days() + 1
    }
}

/// Parse a date range string in START:END format (YYYY-MM-DD:YYYY-MM-DD).
///
/// # Examples
/// ```
/// let range = parse_date_range("2025-01-01:2025-01-03").unwrap();
/// assert_eq!(range.day_count(), 3);
/// ```
pub fn parse_date_range(s: &str) -> Result<DateRange, String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(
            "Date range must be in START:END format (e.g., 2025-01-01:2025-02-05)".to_string(),
        );
    }

    let start = NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
        .map_err(|_| format!("Invalid start date '{}'. Use YYYY-MM-DD format.", parts[0]))?;
    let end = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d")
        .map_err(|_| format!("Invalid end date '{}'. Use YYYY-MM-DD format.", parts[1]))?;

    if end < start {
        return Err(format!(
            "End date ({}) must be on or after start date ({})",
            end, start
        ));
    }

    Ok(DateRange { start, end })
}

/// Generate all dates in a range (inclusive), in chronological order.
pub fn generate_dates(range: &DateRange) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut current = range.start;
    while current <= range.end {
        dates.push(current);
        current += Duration::days(1);
    }
    dates
}

/// Progress tracker for batch runs.
#[derive(Debug)]
pub struct BatchProgress {
    pub current: usize,
    pub total: usize,
    pub current_date: String,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
}

impl BatchProgress {
    pub fn new(total: usize) -> Self {
        Self {
            current: 0,
            total,
            current_date: String::new(),
            passed: 0,
            failed: 0,
            errors: 0,
        }
    }

    pub fn advance(&mut self, date: &str) {
        self.current += 1;
        self.current_date = date.to_string();
    }

    pub fn record_result(&mut self, exit_code: i32) {
        match exit_code {
            0 => self.passed += 1,
            1 => self.failed += 1,
            _ => self.errors += 1,
        }
    }

    pub fn print_progress(&self) {
        println!(
            "\n[{}/{}] Processing {} ...",
            self.current, self.total, self.current_date
        );
    }

    pub fn print_summary(&self) {
        println!("\n========================================");
        println!("BATCH HISTORICAL RUN SUMMARY");
        println!("========================================");
        println!("Total runs:  {}", self.total);
        println!("Passed:      {} (all tests passed)", self.passed);
        println!("Failed:      {} (some tests failed)", self.failed);
        println!("Errors:      {} (build/deploy errors)", self.errors);
        println!("========================================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_date_range() {
        let range = parse_date_range("2025-01-01:2025-01-03").unwrap();
        assert_eq!(range.start.to_string(), "2025-01-01");
        assert_eq!(range.end.to_string(), "2025-01-03");
        assert_eq!(range.day_count(), 3);
    }

    #[test]
    fn test_parse_invalid_format() {
        let result = parse_date_range("invalid");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("START:END"));
    }

    #[test]
    fn test_parse_invalid_start_date() {
        let result = parse_date_range("not-a-date:2025-01-03");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid start date"));
    }

    #[test]
    fn test_parse_invalid_end_date() {
        let result = parse_date_range("2025-01-01:not-a-date");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid end date"));
    }

    #[test]
    fn test_parse_end_before_start() {
        let result = parse_date_range("2025-02-01:2025-01-01");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be on or after"));
    }

    #[test]
    fn test_generate_dates() {
        let range = parse_date_range("2025-01-01:2025-01-03").unwrap();
        let dates = generate_dates(&range);
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[0].to_string(), "2025-01-01");
        assert_eq!(dates[1].to_string(), "2025-01-02");
        assert_eq!(dates[2].to_string(), "2025-01-03");
    }

    #[test]
    fn test_batch_progress() {
        let mut progress = BatchProgress::new(3);
        assert_eq!(progress.current, 0);
        assert_eq!(progress.total, 3);

        progress.advance("2025-01-01");
        assert_eq!(progress.current, 1);
        assert_eq!(progress.current_date, "2025-01-01");

        progress.record_result(0);
        assert_eq!(progress.passed, 1);

        progress.record_result(1);
        assert_eq!(progress.failed, 1);

        progress.record_result(2);
        assert_eq!(progress.errors, 1);
    }
}

use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParserThreadRequest {
    #[default]
    Auto,
    Count(NonZeroUsize),
}

impl ParserThreadRequest {
    pub fn requested(self) -> String {
        match self {
            Self::Auto => "auto".to_owned(),
            Self::Count(count) => count.to_string(),
        }
    }

    pub fn requested_count(self) -> usize {
        match self {
            Self::Auto => std::thread::available_parallelism()
                .unwrap_or(NonZeroUsize::MIN)
                .get(),
            Self::Count(count) => count.get(),
        }
    }

    pub fn resolve(self, useful_ranges: NonZeroUsize) -> ParserThreadSelection {
        let effective = self
            .requested_count()
            .min(useful_ranges.get())
            .min(libpff_sys::MAX_PARALLEL_SCAN_THREADS);
        ParserThreadSelection {
            requested: self.requested(),
            effective,
        }
    }
}

impl fmt::Display for ParserThreadRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.requested())
    }
}

impl FromStr for ParserThreadRequest {
    type Err = ParserThreadParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "auto" {
            return Ok(Self::Auto);
        }
        let count = value
            .parse::<usize>()
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| ParserThreadParseError(value.to_owned()))?;
        Ok(Self::Count(count))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("parser threads must be `auto` or a positive integer, not {0:?}")]
pub struct ParserThreadParseError(String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParserThreadSelection {
    pub requested: String,
    pub effective: usize,
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::ParserThreadRequest;

    #[test]
    fn parser_thread_request_accepts_auto_and_positive_counts() {
        assert_eq!(
            "auto".parse::<ParserThreadRequest>(),
            Ok(ParserThreadRequest::Auto)
        );
        assert_eq!(
            "16".parse::<ParserThreadRequest>(),
            Ok(ParserThreadRequest::Count(
                NonZeroUsize::new(16).expect("non-zero test value")
            ))
        );
        assert!("0".parse::<ParserThreadRequest>().is_err());
        assert!("-1".parse::<ParserThreadRequest>().is_err());
        assert!("many".parse::<ParserThreadRequest>().is_err());
    }

    #[test]
    fn explicit_thread_count_is_capped_by_useful_ranges() {
        let request =
            ParserThreadRequest::Count(NonZeroUsize::new(16).expect("non-zero test thread count"));
        let selection = request.resolve(NonZeroUsize::new(3).expect("non-zero test range count"));
        assert_eq!(selection.requested, "16");
        assert_eq!(selection.effective, 3);
    }

    #[test]
    fn explicit_thread_count_is_capped_by_the_resource_ceiling() {
        let request = ParserThreadRequest::Count(
            NonZeroUsize::new(usize::MAX).expect("non-zero test thread count"),
        );
        let selection =
            request.resolve(NonZeroUsize::new(usize::MAX).expect("non-zero test range count"));
        assert_eq!(selection.effective, libpff_sys::MAX_PARALLEL_SCAN_THREADS);
    }
}

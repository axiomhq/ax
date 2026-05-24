//! Reduction (`Agg`) over a series' `y` values plus the shared
//! numeric-formatting helper.
//!
//! `Agg` is exposed publicly because callers parse `agg=` pragma
//! options at the same time they parse other viz options. Every kind
//! that consumes an aggregate (statistic, top_list, pie, table) goes
//! through `Agg::apply` here.

/// Reduction over a series' `y` values. Surfaces as the `agg=` option on
/// `statistic` and `top_list` pragmas. NaN/infinite values are skipped
/// in every branch so a single bad point can't poison the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agg {
    Last,
    First,
    Avg,
    Sum,
    Min,
    Max,
    Count,
}

impl Agg {
    /// Apply this reduction. Returns `None` for an empty input or when
    /// every point is NaN/infinite (or hidden by the caller).
    pub fn apply(self, points: &[(f64, f64)]) -> Option<f64> {
        // Iterator over finite `y` values, preserving input order.
        let finite = || points.iter().map(|p| p.1).filter(|y| y.is_finite());
        match self {
            // Last: walk backwards over the slice to avoid scanning the whole
            // iterator just to take the final element.
            Agg::Last => points.iter().rev().map(|p| p.1).find(|y| y.is_finite()),
            Agg::First => finite().next(),
            Agg::Sum => {
                let mut any = false;
                let mut s = 0.0;
                for y in finite() {
                    s += y;
                    any = true;
                }
                any.then_some(s)
            }
            Agg::Avg => {
                let mut n = 0usize;
                let mut s = 0.0;
                for y in finite() {
                    s += y;
                    n += 1;
                }
                (n > 0).then(|| s / n as f64)
            }
            Agg::Min => finite().reduce(f64::min),
            Agg::Max => finite().reduce(f64::max),
            Agg::Count => {
                // Count is well-defined even when no points are finite: zero.
                Some(finite().count() as f64)
            }
        }
    }

    /// Parse from the lower-case identifier used in pragmas (`agg=last`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "last" => Agg::Last,
            "first" => Agg::First,
            "avg" | "mean" => Agg::Avg,
            "sum" => Agg::Sum,
            "min" => Agg::Min,
            "max" => Agg::Max,
            "count" => Agg::Count,
            _ => return None,
        })
    }
}

pub(super) fn agg_label(a: Agg) -> &'static str {
    match a {
        Agg::Last => "last",
        Agg::First => "first",
        Agg::Avg => "avg",
        Agg::Sum => "sum",
        Agg::Min => "min",
        Agg::Max => "max",
        Agg::Count => "count",
    }
}

/// Format a single number for display. Uses scientific notation outside
/// the `[1e-2, 1e6)` band so giant counters and tiny rates both stay
/// readable. The optional `unit` is appended verbatim with one space.
pub(super) fn format_value(v: f64, decimals: usize, unit: Option<&str>) -> String {
    let body = if v.abs() >= 1e6 || (v != 0.0 && v.abs() < 1e-2) {
        format!("{v:.*e}", decimals)
    } else {
        format!("{v:.*}", decimals)
    };
    match unit {
        Some(u) => format!("{body} {u}"),
        None => body,
    }
}

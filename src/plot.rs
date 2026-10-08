use std::path::Path;
use std::rc::Rc;
use std::sync::Once;

use anyhow::{Context, Result};
use clap::ValueEnum;
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::{register_font, FontStyle};
use time::OffsetDateTime;

use crate::stats::{Counts, LangTotals};

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 600;
const ONE_DAY: i64 = 86_400;
const OTHER: &str = "Other";

// Fixed categorical order; a series keeps its slot by rank at the newest commit.
const PALETTE: [RGBColor; 8] = [
    RGBColor(0x2a, 0x78, 0xd6),
    RGBColor(0xeb, 0x68, 0x34),
    RGBColor(0x1b, 0xaf, 0x7a),
    RGBColor(0xed, 0xa1, 0x00),
    RGBColor(0xe8, 0x7b, 0xa4),
    RGBColor(0x00, 0x83, 0x00),
    RGBColor(0x4a, 0x3a, 0xa7),
    RGBColor(0xe3, 0x49, 0x48),
];
const OTHER_COLOR: RGBColor = RGBColor(0x8a, 0x8a, 0x85);

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Metric {
    Code,
    Comments,
    Blanks,
    Files,
    Lines,
}

impl Metric {
    pub fn pick(self, c: &Counts) -> u64 {
        match self {
            Metric::Code => c.code,
            Metric::Comments => c.comments,
            Metric::Blanks => c.blanks,
            Metric::Files => c.files,
            Metric::Lines => c.code + c.comments + c.blanks,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Metric::Code => "code",
            Metric::Comments => "comments",
            Metric::Blanks => "blanks",
            Metric::Files => "files",
            Metric::Lines => "lines",
        }
    }
}

#[derive(Default)]
pub struct Collector {
    commits: Vec<(i64, Rc<LangTotals>)>,
}

impl Collector {
    pub fn push(&mut self, seconds: i64, totals: Rc<LangTotals>) {
        self.commits.push((seconds, totals));
    }

    pub fn render(&self, metric: Metric, top: usize, path: &Path) -> Result<()> {
        render(&shape(&self.commits, metric, top), path)
    }
}

pub struct Shaped {
    metric: Metric,
    times: Vec<i64>,
    /// Ordered by value at the newest commit, descending; `Other` last.
    series: Vec<(String, Vec<u64>)>,
}

impl Shaped {
    /// `stacks[i][t]` is the sum of `series[0..=i]` at column `t`.
    fn stacks(&self) -> Vec<Vec<u64>> {
        let mut acc = vec![0u64; self.times.len()];
        self.series
            .iter()
            .map(|(_, values)| {
                for (a, v) in acc.iter_mut().zip(values) {
                    *a += v;
                }
                acc.clone()
            })
            .collect()
    }
}

pub fn shape(commits: &[(i64, Rc<LangTotals>)], metric: Metric, top: usize) -> Shaped {
    let mut order: Vec<usize> = (0..commits.len()).collect();
    order.sort_by_key(|&i| commits[i].0);
    let times: Vec<i64> = order.iter().map(|&i| commits[i].0).collect();

    let mut by_lang: std::collections::BTreeMap<&str, Vec<u64>> = Default::default();
    for (col, &i) in order.iter().enumerate() {
        for (lang, counts) in commits[i].1.iter() {
            by_lang
                .entry(lang.name())
                .or_insert_with(|| vec![0; commits.len()])[col] = metric.pick(counts);
        }
    }

    let mut series: Vec<(String, Vec<u64>)> = by_lang
        .into_iter()
        .map(|(name, values)| (name.to_string(), values))
        .collect();
    series.sort_by(|a, b| {
        let last = |v: &Vec<u64>| v.last().copied().unwrap_or(0);
        last(&b.1).cmp(&last(&a.1)).then_with(|| a.0.cmp(&b.0))
    });

    if series.len() > top {
        let tail = series.split_off(top);
        let mut other = vec![0u64; commits.len()];
        for (_, values) in &tail {
            for (o, v) in other.iter_mut().zip(values) {
                *o += v;
            }
        }
        series.push((OTHER.to_string(), other));
    }

    Shaped {
        metric,
        times,
        series,
    }
}

fn x_range(times: &[i64]) -> (i64, i64) {
    let min = times.iter().copied().min().unwrap_or(0);
    let max = times.iter().copied().max().unwrap_or(0);
    if min == max {
        (min - ONE_DAY, max + ONE_DAY)
    } else {
        (min, max)
    }
}

fn date_label(seconds: i64) -> String {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map(|t| t.date().to_string())
        .unwrap_or_else(|_| seconds.to_string())
}

fn is_svg(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

pub fn render(shaped: &Shaped, path: &Path) -> Result<()> {
    static FONT: Once = Once::new();
    // plotters resolves the family when a text style is built, so this must run
    // before any ChartBuilder call; the default label font is "sans-serif".
    FONT.call_once(|| {
        register_font(
            "sans-serif",
            FontStyle::Normal,
            include_bytes!("../assets/fonts/DejaVuSans.ttf"),
        )
        .unwrap_or_else(|_| panic!("embedded DejaVu Sans failed to parse"));
    });

    let result = if is_svg(path) {
        draw(
            SVGBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area(),
            shaped,
        )
    } else {
        draw(
            BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area(),
            shaped,
        )
    };
    result.with_context(|| format!("failed to write chart to '{}'", path.display()))
}

fn draw<DB>(root: DrawingArea<DB, Shift>, shaped: &Shaped) -> Result<()>
where
    DB: DrawingBackend,
    DB::ErrorType: 'static,
{
    let stacks = shaped.stacks();
    let (x_min, x_max) = x_range(&shaped.times);
    let peak = stacks
        .last()
        .and_then(|s| s.iter().copied().max())
        .unwrap_or(0);
    let y_max = (peak + peak / 20).max(1);
    let title = format!(
        "{} by language over {} commits",
        shaped.metric.label(),
        shaped.times.len()
    );

    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(12)
        .x_label_area_size(36)
        .y_label_area_size(72)
        .build_cartesian_2d(x_min..x_max, 0u64..y_max)?;
    chart
        .configure_mesh()
        .x_labels(8)
        .x_label_formatter(&|s| date_label(*s))
        .y_desc(shaped.metric.label())
        .light_line_style(RGBColor(0xee, 0xee, 0xee))
        .draw()?;

    // Largest cumulative layer first; each lower layer paints over the one above.
    for (i, (name, _)) in shaped.series.iter().enumerate().rev() {
        let color = if name == OTHER {
            OTHER_COLOR
        } else {
            PALETTE[i % PALETTE.len()]
        };
        let points = shaped.times.iter().copied().zip(stacks[i].iter().copied());
        chart
            .draw_series(AreaSeries::new(points, 0, color.mix(0.85)).border_style(color))?
            .label(name)
            .legend(move |(x, y)| Rectangle::new([(x, y - 5), (x + 12, y + 5)], color.filled()));
    }
    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperLeft)
        .background_style(WHITE.mix(0.9))
        .border_style(RGBColor(0xcc, 0xcc, 0xcc))
        .draw()?;
    root.present()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokei::LanguageType;

    fn totals(entries: &[(LanguageType, u64)]) -> Rc<LangTotals> {
        Rc::new(
            entries
                .iter()
                .map(|&(lang, code)| {
                    (
                        lang,
                        Counts {
                            files: 1,
                            code,
                            comments: 2,
                            blanks: 3,
                        },
                    )
                })
                .collect(),
        )
    }

    fn names(shaped: &Shaped) -> Vec<&str> {
        shaped.series.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn orders_by_final_value_desc_and_folds_tail_into_other() {
        let commits = vec![
            (
                100,
                totals(&[
                    (LanguageType::Rust, 10),
                    (LanguageType::Python, 50),
                    (LanguageType::Json, 5),
                ]),
            ),
            (
                200,
                totals(&[
                    (LanguageType::Rust, 100),
                    (LanguageType::Python, 60),
                    (LanguageType::Json, 7),
                ]),
            ),
        ];
        let shaped = shape(&commits, Metric::Code, 1);
        assert_eq!(names(&shaped), ["Rust", OTHER]);
        assert_eq!(shaped.series[0].1, [10, 100]);
        assert_eq!(shaped.series[1].1, [55, 67]);
        assert_eq!(shaped.stacks()[1], [65, 167]);

        let shaped = shape(&commits, Metric::Code, 3);
        assert_eq!(names(&shaped), ["Rust", "Python", "JSON"]);
    }

    #[test]
    fn equal_timestamps_keep_commit_order() {
        let commits = vec![
            (100, totals(&[(LanguageType::Rust, 1)])),
            (100, totals(&[(LanguageType::Rust, 2)])),
            (50, totals(&[(LanguageType::Rust, 3)])),
        ];
        let shaped = shape(&commits, Metric::Code, 8);
        assert_eq!(shaped.times, [50, 100, 100]);
        assert_eq!(shaped.series[0].1, [3, 1, 2]);
    }

    #[test]
    fn lines_metric_sums_code_comments_blanks() {
        let commits = vec![(1, totals(&[(LanguageType::Rust, 10)]))];
        let shaped = shape(&commits, Metric::Lines, 8);
        assert_eq!(shaped.series[0].1, [15]);
        assert_eq!(shape(&commits, Metric::Files, 8).series[0].1, [1]);
    }

    #[test]
    fn empty_tree_commit_yields_zero_column() {
        let commits = vec![
            (1, totals(&[(LanguageType::Rust, 10)])),
            (2, Rc::new(LangTotals::new())),
            (3, totals(&[(LanguageType::Rust, 12)])),
        ];
        let shaped = shape(&commits, Metric::Code, 8);
        assert_eq!(shaped.times, [1, 2, 3]);
        assert_eq!(shaped.series[0].1, [10, 0, 12]);
    }

    #[test]
    fn single_commit_widens_x_range_by_a_day() {
        assert_eq!(x_range(&[1_000]), (1_000 - ONE_DAY, 1_000 + ONE_DAY));
        assert_eq!(x_range(&[1_000, 5_000]), (1_000, 5_000));
    }

    #[test]
    fn date_labels_are_iso_dates() {
        assert_eq!(date_label(1_704_110_400), "2024-01-01");
        assert!(is_svg(Path::new("x.SVG")));
        assert!(!is_svg(Path::new("x.png")));
    }
}

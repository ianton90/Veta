//! A column's distribution drawn in the Statistics tab: a histogram, and
//! below it a violin plot (the smoothed density mirrored, with the
//! quartiles as a box and the median as a dot). Drawn with rectangles
//! only, so no drawing library is needed.

use iced::advanced::Renderer as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, Widget};
use iced::{Border, Color, Element, Length, Rectangle, Renderer, Size, Theme, mouse};
use veta_core::stats::Distribution;

use crate::settings::theme::Tokens;

const HISTOGRAM_HEIGHT: f32 = 56.0;
const GAP: f32 = 8.0;
const VIOLIN_HEIGHT: f32 = 36.0;

/// The histogram and violin plot of `distribution`.
pub(super) fn distribution_plot<'a, Message: 'a>(
    distribution: &Distribution,
    tokens: Tokens,
) -> Element<'a, Message> {
    Element::new(Plot {
        distribution: distribution.clone(),
        tokens,
    })
}

struct Plot {
    distribution: Distribution,
    tokens: Tokens,
}

impl<Message> Widget<Message, Theme, Renderer> for Plot {
    fn size(&self) -> Size<Length> {
        Size::new(
            Length::Fill,
            Length::Fixed(HISTOGRAM_HEIGHT + GAP + VIOLIN_HEIGHT),
        )
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.resolve(
            Length::Fill,
            HISTOGRAM_HEIGHT + GAP + VIOLIN_HEIGHT,
            Size::ZERO,
        );
        layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let t = &self.tokens;
        let d = &self.distribution;
        let fill = |renderer: &mut Renderer, bounds: Rectangle, color: Color| {
            renderer.fill_quad(
                Quad {
                    bounds,
                    border: Border::default(),
                    ..Quad::default()
                },
                color,
            );
        };

        // Histogram: one bar per bin, the tallest full height.
        let tallest = d.bins.iter().copied().max().unwrap_or(0).max(1) as f32;
        let bar_width = bounds.width / d.bins.len().max(1) as f32;
        let bar_color = Color {
            a: 0.75,
            ..t.primary
        };
        for (i, &count) in d.bins.iter().enumerate() {
            let height = HISTOGRAM_HEIGHT * count as f32 / tallest;
            if height <= 0.0 {
                continue;
            }
            fill(
                renderer,
                Rectangle {
                    x: bounds.x + i as f32 * bar_width,
                    y: bounds.y + HISTOGRAM_HEIGHT - height,
                    width: (bar_width - 1.0).max(1.0),
                    height,
                },
                bar_color,
            );
        }
        fill(
            renderer,
            Rectangle {
                x: bounds.x,
                y: bounds.y + HISTOGRAM_HEIGHT,
                width: bounds.width,
                height: 1.0,
            },
            t.border,
        );

        // Violin: the density mirrored around the middle line.
        let middle = bounds.y + HISTOGRAM_HEIGHT + GAP + VIOLIN_HEIGHT / 2.0;
        let points = d.density.len().max(2);
        let slice = bounds.width / points as f32;
        let violin_color = Color {
            a: 0.35,
            ..t.primary
        };
        for (i, &density) in d.density.iter().enumerate() {
            let half = (VIOLIN_HEIGHT / 2.0) * density as f32;
            if half < 0.5 {
                continue;
            }
            // Whole pixels, so slices neither overlap nor leave gaps.
            let left = (bounds.x + i as f32 * slice).round();
            let right = (bounds.x + (i + 1) as f32 * slice).round();
            fill(
                renderer,
                Rectangle {
                    x: left,
                    y: middle - half,
                    width: right - left,
                    height: 2.0 * half,
                },
                violin_color,
            );
        }
        // The quartiles as a box, the median as a dot.
        let span = (d.high - d.low) as f32;
        let at = |x: f64| bounds.x + bounds.width * ((x - d.low) as f32 / span).clamp(0.0, 1.0);
        let [q1, median, q3] = d.quartiles;
        fill(
            renderer,
            Rectangle {
                x: at(q1),
                y: middle - 3.0,
                width: (at(q3) - at(q1)).max(2.0),
                height: 6.0,
            },
            t.text,
        );
        fill(
            renderer,
            Rectangle {
                x: at(median) - 2.0,
                y: middle - 2.0,
                width: 4.0,
                height: 4.0,
            },
            t.background,
        );
    }
}

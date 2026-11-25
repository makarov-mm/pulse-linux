// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

/// Samples kept and drawn. At one sample per second this is the last minute.
const POINTS: usize = 60;

/// A small line graph of recent values, drawn in the widget's accent colour.
/// A `None` sample leaves a break in the line.
pub struct Sparkline {
    area: gtk::DrawingArea,
    points: Rc<RefCell<VecDeque<Option<f64>>>>,
}

impl Sparkline {
    /// `maximum` fixes the top of the axis (100 for a percentage). Without it the axis follows the data.
    pub fn new(maximum: Option<f64>) -> Self {
        let area = gtk::DrawingArea::builder()
            .content_height(44)
            .hexpand(true)
            .css_classes(["accent"])
            .build();
        let points = Rc::new(RefCell::new(VecDeque::with_capacity(POINTS)));
        area.set_draw_func({
            let points = points.clone();
            move |area, context, width, height| {
                // A failed cairo call only loses this frame of the drawing.
                let _ = draw(area, context, width, height, &points.borrow(), maximum);
            }
        });
        Self { area, points }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn push(&self, value: Option<f64>) {
        let mut points = self.points.borrow_mut();
        if points.len() == POINTS {
            points.pop_front();
        }
        points.push_back(value);
        self.area.queue_draw();
    }
}

fn draw(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    points: &VecDeque<Option<f64>>,
    maximum: Option<f64>,
) -> Result<(), cairo::Error> {
    let (width, height) = (f64::from(width), f64::from(height) - 1.0);
    let top = maximum
        .unwrap_or_else(|| points.iter().flatten().copied().fold(0.0, f64::max) * 1.08)
        .max(1.0);
    let step = width / (POINTS - 1) as f64;
    // The newest sample sits at the right edge.
    let offset = POINTS - points.len();
    let color = area.color();
    let (red, green, blue) = (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()));

    let mut run: Vec<(f64, f64)> = Vec::new();
    for (index, point) in points.iter().map(Some).chain([None]).enumerate() {
        if let Some(Some(value)) = point {
            let y = 1.0 + height * (1.0 - (value / top).clamp(0.0, 1.0));
            run.push(((offset + index) as f64 * step, y));
            continue;
        }
        if let (Some(first), Some(last)) = (run.first().copied(), run.last().copied()) {
            context.move_to(first.0, first.1);
            run.iter().skip(1).for_each(|(x, y)| context.line_to(*x, *y));
            context.set_line_width(1.5);
            context.set_source_rgba(red, green, blue, 1.0);
            context.stroke_preserve()?;
            context.line_to(last.0, height + 1.0);
            context.line_to(first.0, height + 1.0);
            context.close_path();
            context.set_source_rgba(red, green, blue, 0.15);
            context.fill()?;
        }
        run.clear();
    }
    Ok(())
}

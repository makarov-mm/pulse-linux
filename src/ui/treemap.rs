// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, pango};

use crate::format;
use crate::i18n::trn;

/// Tiles drawn individually. Smaller items share one last tile.
const TILE_LIMIT: usize = 60;
const GAP: f64 = 1.0;

pub struct Item {
    pub label: String,
    pub size: u64,
    /// A folder can be opened by clicking its tile.
    pub folder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// A tile and the item it stands for. `None` is the tile for everything past the limit.
type Tile = (Rect, Option<usize>);

/// Called with the position of the item whose folder tile was clicked.
type Activated = Box<dyn Fn(usize)>;

#[derive(Default)]
struct State {
    items: RefCell<Vec<Item>>,
    /// Where the last drawing put each tile, for hit testing.
    tiles: RefCell<Vec<Tile>>,
    activated: RefCell<Option<Activated>>,
}

impl State {
    fn set_items(&self, items: Vec<Item>) {
        // Until the next draw there is no geometry for this set of items.
        self.tiles.borrow_mut().clear();
        self.items.replace(items);
    }
}

/// Shows items as rectangles whose areas are proportional to their sizes.
pub struct Treemap {
    area: gtk::DrawingArea,
    state: Rc<State>,
}

impl Treemap {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder()
            .content_height(200)
            .hexpand(true)
            .has_tooltip(true)
            .build();
        let state = Rc::new(State::default());

        area.set_draw_func({
            let state = state.clone();
            move |area, context, width, height| {
                let tiles = tiles(&state.items.borrow(), f64::from(width), f64::from(height));
                // A failed cairo call only loses this frame of the drawing.
                let _ = draw(area, context, &state.items.borrow(), &tiles);
                state.tiles.replace(tiles);
            }
        });
        area.connect_query_tooltip({
            let state = state.clone();
            move |_, x, y, _, tooltip| {
                let items = state.items.borrow();
                let Some((_, index)) = hit(&state.tiles.borrow(), f64::from(x), f64::from(y)) else {
                    return false;
                };
                let text = match index {
                    Some(index) => {
                        let Some(item) = items.get(index) else { return false };
                        format!("{}\n{}", item.label, format::bytes(item.size as f64))
                    }
                    None => trn(
                        "{0} smaller item",
                        "{0} smaller items",
                        items.len().saturating_sub(TILE_LIMIT),
                    ),
                };
                tooltip.set_text(Some(&text));
                true
            }
        });
        let click = gtk::GestureClick::new();
        click.connect_released({
            let state = state.clone();
            move |_, _, x, y| {
                let index = hit(&state.tiles.borrow(), x, y).and_then(|(_, index)| index);
                let folder = index.filter(|index| state.items.borrow().get(*index).is_some_and(|item| item.folder));
                if let (Some(index), Some(activated)) = (folder, state.activated.borrow().as_ref()) {
                    activated(index);
                }
            }
        });
        area.add_controller(click);
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_items(&self, items: Vec<Item>) {
        self.state.set_items(items);
        self.area.queue_draw();
    }

    /// Calls `activated` with an item's position in the last `set_items` when its folder tile is clicked.
    pub fn connect_activated(&self, activated: impl Fn(usize) + 'static) {
        self.state.activated.replace(Some(Box::new(activated)));
    }
}

fn hit(tiles: &[Tile], x: f64, y: f64) -> Option<Tile> {
    tiles.iter().copied().find(|(rect, _)| rect.contains(x, y))
}

fn tiles(items: &[Item], width: f64, height: f64) -> Vec<Tile> {
    let mut order: Vec<usize> = (0..items.len()).filter(|index| items[*index].size > 0).collect();
    order.sort_by_key(|index| std::cmp::Reverse(items[*index].size));
    let rest: u64 = order.iter().skip(TILE_LIMIT).map(|index| items[*index].size).sum();
    order.truncate(TILE_LIMIT);

    let mut weighted: Vec<(f64, Option<usize>)> = order
        .iter()
        .map(|index| (items[*index].size as f64, Some(*index)))
        .collect();
    if rest > 0 {
        weighted.push((rest as f64, None));
        weighted.sort_by(|a, b| b.0.total_cmp(&a.0));
    }
    let weights: Vec<f64> = weighted.iter().map(|(weight, _)| *weight).collect();
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    squarify(&weights, bounds)
        .into_iter()
        .zip(weighted.into_iter().map(|(_, index)| index))
        .collect()
}

/// Lays out `weights`, largest first, as rectangles that fill `bounds` and stay close to square
/// (Bruls, Huizing and van Wijk, "Squarified Treemaps").
fn squarify(weights: &[f64], mut bounds: Rect) -> Vec<Rect> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }
    let scale = bounds.width * bounds.height / total;
    let areas: Vec<f64> = weights.iter().map(|weight| weight * scale).collect();
    let mut rects = Vec::with_capacity(areas.len());
    let mut start = 0;

    while start < areas.len() {
        // A row runs along the shorter side and takes items while that keeps them squarer.
        let side = bounds.width.min(bounds.height);
        let worst = |sum: f64, smallest: f64| {
            let (side, sum) = (side * side, sum * sum);
            (side * areas[start] / sum).max(sum / (side * smallest))
        };
        let (mut end, mut sum) = (start + 1, areas[start]);
        while end < areas.len() && worst(sum + areas[end], areas[end]) <= worst(sum, areas[end - 1]) {
            sum += areas[end];
            end += 1;
        }

        let thickness = sum / side;
        let wide = bounds.width >= bounds.height;
        let mut offset = 0.0;
        for area in &areas[start..end] {
            let length = area / thickness;
            rects.push(if wide {
                Rect {
                    x: bounds.x,
                    y: bounds.y + offset,
                    width: thickness,
                    height: length,
                }
            } else {
                Rect {
                    x: bounds.x + offset,
                    y: bounds.y,
                    width: length,
                    height: thickness,
                }
            });
            offset += length;
        }
        if wide {
            bounds.x += thickness;
            bounds.width -= thickness;
        } else {
            bounds.y += thickness;
            bounds.height -= thickness;
        }
        start = end;
    }
    rects
}

fn draw(area: &gtk::DrawingArea, context: &cairo::Context, items: &[Item], tiles: &[Tile]) -> Result<(), cairo::Error> {
    let text = area.color();
    // Libadwaita defines the accent as a named colour. The fallback is its default blue.
    #[allow(deprecated)]
    let accent = area
        .style_context()
        .lookup_color("accent_bg_color")
        .unwrap_or_else(|| gdk::RGBA::new(0.21, 0.52, 0.89, 1.0));
    let layout = area.create_pango_layout(None);
    layout.set_ellipsize(pango::EllipsizeMode::End);

    for (rect, index) in tiles {
        let (width, height) = (rect.width - GAP, rect.height - GAP);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let item = index.and_then(|index| items.get(index));
        // Folders are the stronger tiles. Files and the remainder recede.
        let strength = if item.is_some_and(|item| item.folder) {
            0.55
        } else {
            0.25
        };
        context.rectangle(rect.x, rect.y, width, height);
        context.set_source_rgba(
            accent.red().into(),
            accent.green().into(),
            accent.blue().into(),
            strength,
        );
        context.fill()?;

        let line = f64::from(layout.pixel_size().1.max(14));
        if width < 48.0 || height < line + 6.0 {
            continue;
        }
        let name = item.map_or("…", |item| item.label.as_str());
        let label = match item {
            Some(item) if height >= 2.0 * line + 8.0 => format!("{name}\n{}", format::bytes(item.size as f64)),
            _ => name.to_owned(),
        };
        layout.set_text(&label);
        layout.set_width(((width - 10.0) * f64::from(pango::SCALE)) as i32);
        layout.set_height(((height - 6.0) * f64::from(pango::SCALE)) as i32);
        context.set_source_rgba(text.red().into(), text.green().into(), text.blue().into(), 1.0);
        context.move_to(rect.x + 5.0, rect.y + 3.0);
        pangocairo::functions::show_layout(context, &layout);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_folders_invalidates_hit_targets_before_the_next_draw() {
        let state = State::default();
        state.set_items(vec![Item {
            label: "old".into(),
            size: 100,
            folder: true,
        }]);
        state.tiles.replace(tiles(&state.items.borrow(), 100.0, 100.0));
        assert!(hit(&state.tiles.borrow(), 50.0, 50.0).is_some());
        state.set_items(Vec::new());
        assert!(hit(&state.tiles.borrow(), 50.0, 50.0).is_none());
    }

    #[test]
    fn rectangles_fill_the_bounds_in_proportion() {
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 600.0,
            height: 400.0,
        };
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&weights, bounds);
        assert_eq!(rects.len(), weights.len());
        let unit = bounds.width * bounds.height / weights.iter().sum::<f64>();
        for (rect, weight) in rects.iter().zip(weights) {
            assert!((rect.width * rect.height - weight * unit).abs() < 1e-6);
            assert!(rect.x >= -1e-9 && rect.y >= -1e-9);
            assert!(rect.x + rect.width <= bounds.width + 1e-6 && rect.y + rect.height <= bounds.height + 1e-6);
        }
        // The paper's example starts with the two largest items stacked in the first column.
        assert_eq!(
            rects[0],
            Rect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 200.0
            }
        );
    }

    #[test]
    fn nothing_is_laid_out_without_area_or_weight() {
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 100.0,
        };
        assert!(squarify(&[1.0], bounds).is_empty());
        assert!(squarify(&[], Rect { width: 10.0, ..bounds }).is_empty());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;

use gtk::prelude::*;

use super::metric_card::MetricCard;
use crate::format;
use crate::telemetry::{DeviceSample, Frame};

/// What one tile shows for one sample.
struct Reading {
    name: String,
    value: Option<f64>,
    text: String,
    caption: String,
}

/// A titled grid of tiles, one per device. Tiles appear and disappear with their devices.
struct Group {
    root: gtk::Box,
    flow: gtk::FlowBox,
    maximum: Option<f64>,
    tiles: RefCell<Vec<(String, MetricCard)>>,
}

impl Group {
    fn new(title: &str, maximum: Option<f64>) -> Self {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(8)
            .column_spacing(12)
            .row_spacing(12)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .visible(false)
            .build();
        root.append(&heading);
        root.append(&flow);
        Self {
            root,
            flow,
            maximum,
            tiles: RefCell::default(),
        }
    }

    fn sync(&self, readings: Vec<Reading>) {
        let mut tiles = self.tiles.borrow_mut();
        tiles.retain(|(name, tile)| {
            let present = readings.iter().any(|reading| reading.name == *name);
            if !present {
                // The flow box wraps each tile in a child of its own, which is what it removes.
                if let Some(child) = tile.widget().parent() {
                    self.flow.remove(&child);
                }
            }
            present
        });
        for reading in &readings {
            let index = tiles
                .iter()
                .position(|(name, _)| *name == reading.name)
                .unwrap_or_else(|| {
                    let tile = MetricCard::compact(&reading.name, self.maximum);
                    self.flow.append(tile.widget());
                    tiles.push((reading.name.clone(), tile));
                    tiles.len() - 1
                });
            tiles[index].1.update(reading.value, &reading.text, &reading.caption);
        }
        self.root.set_visible(!tiles.is_empty());
    }
}

/// Load per processor, per disk and per network interface.
pub struct DevicesPage {
    root: gtk::ScrolledWindow,
    processors: Group,
    disks: Group,
    interfaces: Group,
}

impl DevicesPage {
    pub fn new() -> Self {
        let processors = Group::new("Processors", Some(100.0));
        let disks = Group::new("Disks", None);
        let interfaces = Group::new("Network Interfaces", None);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        for group in [&processors, &disks, &interfaces] {
            content.append(&group.root);
        }
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();
        Self {
            root,
            processors,
            disks,
            interfaces,
        }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn update(&self, frame: &Frame) {
        self.processors.sync(
            frame
                .cores
                .iter()
                .map(|core| Reading {
                    name: format!("CPU {}", core.number),
                    value: core.usage,
                    text: format::percent(core.usage),
                    caption: String::new(),
                })
                .collect(),
        );
        self.disks.sync(transfers(&frame.disks, "Read", "Write"));
        self.interfaces.sync(transfers(&frame.interfaces, "↓", "↑"));
    }
}

fn transfers(devices: &[DeviceSample], incoming: &str, outgoing: &str) -> Vec<Reading> {
    devices
        .iter()
        .map(|device| {
            let total = device.incoming.zip(device.outgoing).map(|(a, b)| a + b);
            Reading {
                name: device.name.clone(),
                value: total,
                text: format::rate(total),
                caption: format!(
                    "{incoming} {} · {outgoing} {}",
                    format::rate(device.incoming),
                    format::rate(device.outgoing)
                ),
            }
        })
        .collect()
}

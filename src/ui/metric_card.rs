// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::pango;
use gtk::prelude::*;

use super::sparkline::Sparkline;

/// One headline metric: its current value, a line of detail and a graph of the last minute.
pub struct MetricCard {
    root: gtk::Box,
    value: gtk::Label,
    caption: gtk::Label,
    graph: Sparkline,
}

impl MetricCard {
    pub fn new(title: &str, maximum: Option<f64>) -> Self {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        let value = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["title-2", "numeric"])
            .build();
        let caption = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build();
        let graph = Sparkline::new(maximum);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        content.append(&heading);
        content.append(&value);
        content.append(graph.widget());
        content.append(&caption);

        let root = gtk::Box::builder().css_classes(["card"]).hexpand(true).build();
        root.append(&content);
        Self { root, value, caption, graph }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// `value` goes to the graph. `text` and `caption` are shown as given.
    pub fn update(&self, value: Option<f64>, text: &str, caption: &str) {
        self.graph.push(value);
        self.value.set_label(text);
        self.caption.set_label(caption);
        self.caption.set_tooltip_text(Some(caption));
    }
}

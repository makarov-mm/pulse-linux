// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::pango;
use gtk::prelude::*;

use super::sparkline::Sparkline;

/// One metric: its current value, a line of detail and a graph of the last minute.
pub struct MetricCard {
    root: gtk::Box,
    value: gtk::Label,
    caption: gtk::Label,
    graph: Sparkline,
}

impl MetricCard {
    /// A headline card. `maximum` fixes the top of the graph; without it the graph follows the data.
    pub fn new(title: &str, maximum: Option<f64>) -> Self {
        Self::build(title, maximum, false)
    }

    /// A smaller card for one device among many.
    pub fn compact(title: &str, maximum: Option<f64>) -> Self {
        Self::build(title, maximum, true)
    }

    fn build(title: &str, maximum: Option<f64>, compact: bool) -> Self {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        let value = gtk::Label::builder()
            .xalign(0.0)
            .css_classes([if compact { "heading" } else { "title-2" }, "numeric"])
            .build();
        let caption = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build();
        let graph = Sparkline::new(maximum, if compact { 30 } else { 44 });

        let margin = if compact { 10 } else { 14 };
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(margin - 2)
            .margin_bottom(margin - 2)
            .margin_start(margin)
            .margin_end(margin)
            .build();
        content.append(&heading);
        content.append(&value);
        content.append(graph.widget());
        content.append(&caption);

        let root = gtk::Box::builder().css_classes(["card"]).hexpand(true).build();
        root.append(&content);
        Self {
            root,
            value,
            caption,
            graph,
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// `value` goes to the graph. `text` and `caption` are shown as given; an empty caption takes no room.
    pub fn update(&self, value: Option<f64>, text: &str, caption: &str) {
        self.graph.push(value);
        self.value.set_label(text);
        self.caption.set_label(caption);
        self.caption.set_visible(!caption.is_empty());
        self.caption.set_tooltip_text((!caption.is_empty()).then_some(caption));
    }
}

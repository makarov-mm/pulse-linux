// SPDX-License-Identifier: GPL-3.0-or-later

//! Column builders shared by the tables whose cells show string properties as they are.

use gtk::prelude::*;
use gtk::{glib, pango};

fn expression<T: IsA<glib::Object>>(property: &str) -> gtk::PropertyExpression {
    gtk::PropertyExpression::new(T::static_type(), gtk::Expression::NONE, property)
}

fn column<T: IsA<glib::Object>>(
    title: &str,
    property: &'static str,
    right_aligned: bool,
    expand: bool,
    sorter: &impl IsA<gtk::Sorter>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(if right_aligned { 1.0 } else { 0.0 })
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        if right_aligned {
            label.add_css_class("numeric");
        }
        item.set_child(Some(&label));
        item.property_expression("item")
            .chain_property::<T>(property)
            .bind(&label, "label", gtk::Widget::NONE);
    });
    gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .sorter(sorter)
        .expand(expand)
        .resizable(true)
        .build()
}

/// A left-aligned column that shows the string property `property` of `T` and sorts by it.
pub fn text_column<T: IsA<glib::Object>>(title: &str, property: &'static str, expand: bool) -> gtk::ColumnViewColumn {
    let sorter = gtk::StringSorter::new(Some(expression::<T>(property)));
    column::<T>(title, property, false, expand, &sorter)
}

/// A right-aligned column that shows the string property `text` of `T` and sorts by the numeric property `value`.
pub fn number_column<T: IsA<glib::Object>>(
    title: &str,
    text: &'static str,
    value: &'static str,
) -> gtk::ColumnViewColumn {
    let sorter = gtk::NumericSorter::new(Some(expression::<T>(value)));
    column::<T>(title, text, true, false, &sorter)
}

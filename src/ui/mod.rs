// SPDX-License-Identifier: GPL-3.0-or-later

mod connections_page;
mod devices_page;
mod metric_card;
mod process_details;
mod process_list;
mod space_page;
mod sparkline;
mod startup_page;
mod table;
mod treemap;

pub use connections_page::ConnectionsPage;
pub use devices_page::DevicesPage;
pub use metric_card::MetricCard;
pub use process_details::present as present_process_details;
pub use process_list::{ProcessList, ProcessObject};
pub use space_page::SpacePage;
pub use startup_page::StartupPage;

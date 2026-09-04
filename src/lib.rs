pub mod utils;
#[macro_use]
pub mod macros;
#[macro_use]
extern crate log;

mod app;
mod backdrop;
mod badge;
mod config;
mod foreground;
mod icon_cache;
mod icon_loader;
mod keyboard;
mod layout;
mod metrics;
mod painter;
mod painter_resources;
mod startup;
mod trayicon;

pub use crate::app::start;
pub use crate::config::{
    load_config, AppearanceConfig, BackdropFallback, BackdropMode, BackgroundColor, Config,
    LayoutMode, MonitorTarget,
};

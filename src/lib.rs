pub mod utils;
#[macro_use]
pub mod macros;
#[macro_use]
extern crate log;

mod accessibility;
mod app;
mod app_identity;
mod app_name;
mod appearance;
mod badge;
mod config;
mod diagnostics;
mod font_resources;
mod foreground;
mod icon_cache;
mod icon_loader;
mod keyboard;
mod layout;
mod localization;
mod monitor_scope;
mod mru;
mod painter;
mod pause;
mod picker;
mod pixels;
mod preview;
mod process_metadata;
mod render_surface;
mod restart;
mod search;
mod startup;
mod text_raster;
mod trayicon;
mod window_details;
mod window_snapshot;
mod window_target;
mod worker;

pub use crate::app::{run, start};
pub use crate::config::{load_config, prepare_log_file, Config, LoadedConfig};

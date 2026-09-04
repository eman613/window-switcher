pub mod utils;
#[macro_use]
pub mod macros;
#[macro_use]
extern crate log;

mod app;
mod backdrop;
mod badge;
mod config;
mod config_diagnostics;
mod config_file;
mod config_schema;
mod config_watcher;
mod foreground;
mod icon_cache;
mod icon_loader;
mod keyboard;
mod layout;
mod localization;
mod metrics;
mod painter;
mod painter_resources;
mod startup;
mod trayicon;

pub use crate::app::{start, start_with_config, AppExit};
pub use crate::config::{
    AppearanceConfig, BackdropFallback, BackdropMode, BackgroundColor, Config, ConfigReloadMode,
    LayoutMode, MonitorTarget, PerformanceConfig, RenderScale, CURRENT_CONFIG_VERSION,
};
pub use crate::config_diagnostics::{ConfigDiagnostic, ConfigDiagnosticSeverity};
pub use crate::config_file::{load_config, load_config_report, ConfigLoadReport, ConfigSource};
pub use crate::localization::{set_language, text, Language, TextId};

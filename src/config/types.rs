use std::str::FromStr;

use anyhow::{bail, Result};

macro_rules! choices {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl FromStr for $name {
            type Err = anyhow::Error;
            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => bail!(concat!("可选值：", $($value, " "),+)),
                }
            }
        }
    };
}

choices!(WatchMode { Auto => "auto", Notify => "notify", Poll => "poll" });
choices!(StartupEnabled { Auto => "auto", Yes => "yes", No => "no" });
choices!(RunLevel { Inherit => "inherit", Standard => "standard", Highest => "highest" });
choices!(BatteryPolicy { Inherit => "inherit", Allow => "allow", Stop => "stop" });
choices!(Language { Chinese => "zh-CN", English => "en-US", Auto => "auto" });
choices!(MonitorPolicy { Cursor => "cursor", Foreground => "foreground", Primary => "primary" });
choices!(ForegroundPolicy { Passthrough => "passthrough", Handle => "handle" });
choices!(InjectedPolicy { Handle => "handle", Passthrough => "passthrough" });
choices!(RenderScale { Auto => "auto", One => "1", Two => "2", Four => "4", Six => "6" });
choices!(Theme { Auto => "auto", Light => "light", Dark => "dark" });
choices!(AppNameMode { Off => "off", Selected => "selected" });
choices!(SearchMatch { Fuzzy => "fuzzy", Contains => "contains", Prefix => "prefix" });
choices!(SwitchOrder { Existing => "existing", Mru => "mru" });

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use xml::{reader::XmlEvent, EventReader};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskPolicy {
    pub(crate) highest: bool,
    pub(crate) allow_battery: bool,
    pub(crate) stop_on_battery: bool,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskDefinition {
    pub(crate) xml: String,
    pub(crate) command: String,
    pub(crate) user: String,
    pub(crate) policy: TaskPolicy,
}

pub(crate) fn matches_executable(value: &str, executable: &str) -> bool {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    value.replace('/', "\\").to_lowercase() == executable.replace('/', "\\").to_lowercase()
}

impl TaskDefinition {
    pub(crate) fn parse(xml: &str) -> Result<Self> {
        if xml.len() > 1024 * 1024 {
            bail!("计划任务 XML 超过 1 MiB");
        }
        // XML arrives as Unicode BSTR, not an encoded file. Remove its encoding
        // declaration before handing UTF-8 Rust text to the streaming parser.
        let text = xml.trim_start_matches('\u{feff}').trim_start();
        let text = if text.starts_with("<?xml ") {
            &text[text.find("?>").context("计划任务 XML 声明无效")? + 2..]
        } else {
            text
        };
        let mut stack = Vec::new();
        let mut values: HashMap<String, String> = HashMap::new();
        let mut executions = 0;
        let mut principals = 0;
        for event in EventReader::from_str(text) {
            match event.context("计划任务 XML 无效")? {
                XmlEvent::StartElement { name, .. } => {
                    if stack.last().is_some_and(|parent| parent == "Actions")
                        && name.local_name != "Exec"
                    {
                        bail!("计划任务含其他操作，不能认定为本应用入口");
                    }
                    if name.local_name == "Exec" {
                        executions += 1;
                    }
                    if name.local_name == "Principal" {
                        principals += 1;
                    }
                    stack.push(name.local_name);
                    if stack.len() > 32 {
                        bail!("计划任务 XML 嵌套过深");
                    }
                }
                XmlEvent::Characters(value) | XmlEvent::CData(value) => {
                    values.entry(stack.join("/")).or_default().push_str(&value);
                }
                XmlEvent::EndElement { .. } => {
                    stack.pop();
                }
                _ => {}
            }
        }
        if executions != 1 || principals != 1 {
            bail!("计划任务必须只有一个执行操作和用户主体");
        }
        let get = |path: &str| -> Result<String> {
            values
                .get(path)
                .map(|v| v.trim().to_owned())
                .with_context(|| format!("计划任务缺少 {path}"))
        };
        let flag = |name: &str| -> Result<bool> {
            match get(&format!("Task/Settings/{name}"))?.as_str() {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => bail!("计划任务布尔设置无效"),
            }
        };
        if values
            .get("Task/Actions/Exec/Arguments")
            .is_some_and(|v| !v.trim().is_empty())
        {
            bail!("计划任务带有额外启动参数，不能覆盖该入口");
        }
        let highest = match get("Task/Principals/Principal/RunLevel")?.as_str() {
            "HighestAvailable" => true,
            "LeastPrivilege" => false,
            _ => bail!("计划任务运行级别无效"),
        };
        Ok(Self {
            xml: xml.to_owned(),
            command: get("Task/Actions/Exec/Command")?,
            user: get("Task/Principals/Principal/UserId")?,
            policy: TaskPolicy {
                highest,
                allow_battery: !flag("DisallowStartIfOnBatteries")?,
                stop_on_battery: flag("StopIfGoingOnBatteries")?,
                enabled: flag("Enabled")?,
            },
        })
    }

    pub(crate) fn require_owner(&self, executable: &str, user: &str) -> Result<()> {
        if !matches_executable(&self.command, executable) || self.user != user {
            bail!("同名计划任务属于其他程序路径或用户；未修改该任务");
        }
        Ok(())
    }

    pub(crate) fn with_policy(&self, policy: TaskPolicy) -> Result<String> {
        let mut xml = self.xml.clone();
        replace_leaf(
            &mut xml,
            &["Task", "Principals", "Principal"],
            "RunLevel",
            if policy.highest {
                "HighestAvailable"
            } else {
                "LeastPrivilege"
            },
        )?;
        for (key, value) in [
            ("DisallowStartIfOnBatteries", !policy.allow_battery),
            ("StopIfGoingOnBatteries", policy.stop_on_battery),
            ("Enabled", policy.enabled),
        ] {
            replace_leaf(
                &mut xml,
                &["Task", "Settings"],
                key,
                if value { "true" } else { "false" },
            )?;
        }
        Ok(xml)
    }

    pub(crate) fn create(executable: &str, user: &str, policy: TaskPolicy) -> String {
        let executable = escape(executable);
        let user = escape(user);
        let level = if policy.highest {
            "HighestAvailable"
        } else {
            "LeastPrivilege"
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Author>{user}</Author><URI>\WindowSwitcher</URI></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>{level}</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>{disallow}</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>{stop}</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand><Enabled>{enabled}</Enabled><Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle><WakeToRun>false</WakeToRun><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{executable}</Command></Exec></Actions>
</Task>"#,
            disallow = !policy.allow_battery,
            stop = policy.stop_on_battery,
            enabled = policy.enabled
        )
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn element(text: &str, name: &str) -> Result<std::ops::Range<usize>> {
    let open = format!("<{name}");
    let positions: Vec<_> = text
        .match_indices(&open)
        .filter(|(at, _)| {
            text.as_bytes()
                .get(at + open.len())
                .is_some_and(|c| c.is_ascii_whitespace() || *c == b'>')
        })
        .map(|(at, _)| at)
        .collect();
    if positions.len() != 1 {
        bail!("计划任务字段 {name} 不唯一或使用了不支持的 XML 前缀；原任务保留");
    }
    let start = positions[0] + text[positions[0]..].find('>').context("XML 起始标签无效")? + 1;
    let end = start
        + text[start..]
            .find(&format!("</{name}>"))
            .context("XML 结束标签无效")?;
    Ok(start..end)
}

fn replace_leaf(xml: &mut String, parents: &[&str], name: &str, value: &str) -> Result<()> {
    let mut range = 0..xml.len();
    for parent in parents {
        let inner = element(&xml[range.clone()], parent)?;
        range = range.start + inner.start..range.start + inner.end;
    }
    let inner = element(&xml[range.clone()], name)?;
    xml.replace_range(range.start + inner.start..range.start + inner.end, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xml_round_trip_escapes_special_characters_and_preserves_other_settings() {
        let policy = TaskPolicy {
            highest: true,
            allow_battery: true,
            stop_on_battery: true,
            enabled: true,
        };
        let xml = TaskDefinition::create("D:\\A & B\\切换.exe", "S-1-5-21-123", policy);
        let parsed = TaskDefinition::parse(&xml).unwrap();
        parsed
            .require_owner("D:\\A & B\\切换.exe", "S-1-5-21-123")
            .unwrap();
        assert_eq!(parsed.policy, policy);
        let changed = parsed
            .with_policy(TaskPolicy {
                highest: false,
                stop_on_battery: false,
                ..policy
            })
            .unwrap();
        assert!(changed.contains("<AllowHardTerminate>true</AllowHardTerminate>"));
        assert!(changed.contains("<LogonTrigger><Enabled>true</Enabled>"));
        assert!(!TaskDefinition::parse(&changed).unwrap().policy.highest);
        assert!(parsed
            .require_owner("D:\\foreign.exe", "S-1-5-21-123")
            .is_err());
    }
}

use anyhow::{bail, Context, Result};

use crate::{
    config::{BatteryPolicy, Config, RunLevel, StartupEnabled},
    utils::scheduled_task::xml::{matches_executable, TaskDefinition, TaskPolicy},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Run,
    Task,
}

pub(super) trait Backend {
    fn read(&mut self, entry: Entry) -> Result<Option<String>>;
    fn exchange(
        &mut self,
        entry: Entry,
        expected: Option<&str>,
        replacement: Option<&str>,
    ) -> Result<Option<String>>;
    fn current(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    run: Option<String>,
    task: Option<String>,
}

impl Snapshot {
    fn read(backend: &mut impl Backend) -> Result<Self> {
        Ok(Self {
            run: backend.read(Entry::Run)?,
            task: backend.read(Entry::Task)?,
        })
    }
    fn get(&self, entry: Entry) -> Option<&str> {
        match entry {
            Entry::Run => self.run.as_deref(),
            Entry::Task => self.task.as_deref(),
        }
    }
    fn validate(&self, executable: &str, user: &str) -> Result<Option<TaskDefinition>> {
        if self
            .run
            .as_ref()
            .is_some_and(|value| !matches_executable(value, executable))
        {
            bail!("同名 Run 启动项属于其他程序或包含额外参数；未修改系统状态");
        }
        self.task
            .as_ref()
            .map(|xml| {
                let task = TaskDefinition::parse(xml)?;
                task.require_owner(executable, user)?;
                Ok(task)
            })
            .transpose()
    }
    fn enabled(&self, executable: &str, user: &str) -> Result<bool> {
        let task = self.validate(executable, user)?;
        Ok(self.run.is_some() || task.is_some_and(|task| task.policy.enabled))
    }
}

struct Change {
    entry: Entry,
    before: Option<String>,
    after: Option<String>,
}

pub(super) fn reconcile(
    backend: &mut impl Backend,
    configuration: &Config,
    executable: &str,
    user: &str,
    admin: bool,
) -> Result<bool> {
    let before = Snapshot::read(backend)?;
    let old_task = before.validate(executable, user)?;
    let was_enabled = before.enabled(executable, user)?;
    if configuration.startup_enabled == StartupEnabled::Auto
        && configuration.startup_run_level == RunLevel::Inherit
        && configuration.startup_battery_policy == BatteryPolicy::Inherit
    {
        return Ok(was_enabled);
    }
    let enabled = match configuration.startup_enabled {
        StartupEnabled::Auto => was_enabled,
        StartupEnabled::Yes => true,
        StartupEnabled::No => false,
    };
    let mut journal = Vec::new();
    let operation = (|| {
        backend.current()?;
        if enabled {
            let (entry, value) = target(
                configuration,
                &before,
                old_task.as_ref(),
                executable,
                user,
                admin,
            )?;
            change(
                backend,
                &mut journal,
                entry,
                before.get(entry),
                Some(&value),
            )?;
            // The new target has been read back and verified before removing the
            // old entry. Failure below restores old first, then removes new.
            backend.current()?;
            let old = if entry == Entry::Run {
                Entry::Task
            } else {
                Entry::Run
            };
            if before.get(old).is_some() {
                change(backend, &mut journal, old, before.get(old), None)?;
            }
        } else {
            for entry in [Entry::Run, Entry::Task] {
                if before.get(entry).is_some() {
                    change(backend, &mut journal, entry, before.get(entry), None)?;
                }
            }
        }
        backend.current()?;
        let actual = Snapshot::read(backend)?.enabled(executable, user)?;
        if actual != enabled {
            bail!("自启动系统状态与 INI 期望不一致");
        }
        Ok(actual)
    })();
    match operation {
        Ok(enabled) => Ok(enabled),
        Err(error) => match rollback(backend, &journal) {
            Ok(()) => Err(error.context("自启动未应用，已恢复本次修改的入口；INI 期望值保留")),
            Err(restore) => Err(error.context(format!(
                "自启动未应用，恢复尚未完成：{restore:#}；保留现有入口，请检查权限或外部修改"
            ))),
        },
    }
}

fn target(
    configuration: &Config,
    before: &Snapshot,
    old_task: Option<&TaskDefinition>,
    executable: &str,
    user: &str,
    admin: bool,
) -> Result<(Entry, String)> {
    let highest = match configuration.startup_run_level {
        RunLevel::Highest => true,
        RunLevel::Standard => false,
        RunLevel::Inherit => {
            old_task.map_or(before.run.is_none() && admin, |task| task.policy.highest)
        }
    };
    let use_task = highest
        || old_task.is_some()
        || configuration.startup_battery_policy != BatteryPolicy::Inherit;
    if !use_task {
        return Ok((Entry::Run, format!("\"{executable}\"")));
    }
    let (allow_battery, stop_on_battery) = match configuration.startup_battery_policy {
        BatteryPolicy::Allow => (true, false),
        BatteryPolicy::Stop => (false, true),
        BatteryPolicy::Inherit => old_task.map_or((true, false), |task| {
            (task.policy.allow_battery, task.policy.stop_on_battery)
        }),
    };
    let policy = TaskPolicy {
        highest,
        allow_battery,
        stop_on_battery,
        enabled: true,
    };
    if highest && !admin && old_task.is_none_or(|task| task.policy != policy) {
        bail!("最高权限自启动需要以管理员身份运行；不会自动提权或降级已有任务");
    }
    let xml = match old_task {
        Some(task) if task.policy == policy => task.xml.clone(),
        Some(task) => task.with_policy(policy)?,
        None => TaskDefinition::create(executable, user, policy),
    };
    Ok((Entry::Task, xml))
}

fn equivalent(entry: Entry, expected: Option<&str>, actual: Option<&str>) -> Result<bool> {
    if entry == Entry::Run || expected.is_none() || actual.is_none() {
        return Ok(expected == actual);
    }
    let expected = TaskDefinition::parse(expected.unwrap())?;
    let actual = TaskDefinition::parse(actual.unwrap())?;
    Ok(matches_executable(&actual.command, &expected.command)
        && actual.user == expected.user
        && actual.policy == expected.policy)
}

fn change(
    backend: &mut impl Backend,
    journal: &mut Vec<Change>,
    entry: Entry,
    expected: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    if expected == value {
        return Ok(());
    }
    backend.current()?;
    journal.push(Change {
        entry,
        before: expected.map(str::to_owned),
        after: value.map(str::to_owned),
    });
    let actual = backend
        .exchange(entry, expected, value)
        .context("自启动入口修改失败")?;
    if !equivalent(entry, value, actual.as_deref())? {
        bail!("自启动目标入口读回校验失败");
    }
    journal.last_mut().unwrap().after = actual;
    Ok(())
}

fn rollback(backend: &mut impl Backend, journal: &[Change]) -> Result<()> {
    for change in journal.iter().rev() {
        let actual = backend.read(change.entry)?;
        if actual == change.before {
            continue;
        }
        if actual != change.after {
            bail!("入口已被外部修改；未覆盖该变更");
        }
        let restored =
            backend.exchange(change.entry, actual.as_deref(), change.before.as_deref())?;
        if !equivalent(change.entry, change.before.as_deref(), restored.as_deref())? {
            bail!("旧入口恢复校验失败；保留仍可用的新入口");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

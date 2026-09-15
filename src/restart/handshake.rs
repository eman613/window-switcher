use anyhow::Result;

use super::{protocol::Signal, ParentCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Wait(Signal),
    CheckCurrent,
    Suspend,
    WaitUi(ParentCommand),
    Send(Signal),
    Ready,
    Done,
}

/// Kept separate from pipe/process mechanics so every failure boundary can be
/// exercised without installing hooks or changing the user's running instance.
pub(super) fn negotiate(mut perform: impl FnMut(Step) -> Result<()>) -> Result<()> {
    for step in [
        Step::Wait(Signal::Prepared),
        Step::CheckCurrent,
        Step::Suspend,
        Step::WaitUi(ParentCommand::Suspended),
        Step::CheckCurrent,
        Step::Send(Signal::Activate),
        Step::Wait(Signal::Active),
        Step::CheckCurrent,
        Step::Ready,
        Step::WaitUi(ParentCommand::Commit),
        Step::CheckCurrent,
        Step::Send(Signal::Commit),
        Step::Wait(Signal::Committed),
        Step::Done,
    ] {
        perform(step)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failed_preflight_handoff_or_ack_boundary_prevents_success() {
        let failures = [
            Step::Wait(Signal::Prepared),
            Step::Suspend,
            Step::WaitUi(ParentCommand::Suspended),
            Step::Send(Signal::Activate),
            Step::Wait(Signal::Active),
            Step::Ready,
            Step::WaitUi(ParentCommand::Commit),
            Step::Send(Signal::Commit),
            Step::Wait(Signal::Committed),
        ];
        for failed in failures {
            let mut success = false;
            assert!(negotiate(|step| {
                if step == failed {
                    anyhow::bail!("injected peer/resource failure");
                }
                if step == Step::Done {
                    success = true;
                }
                Ok(())
            })
            .is_err());
            assert!(!success, "reported success after {failed:?}");
        }
    }

    #[test]
    fn child_activation_waits_for_quiescence_and_old_exit_waits_for_commit_ack() {
        let mut paused = false;
        let mut active = false;
        let mut committed = false;
        let mut complete = false;
        negotiate(|step| {
            match step {
                Step::WaitUi(ParentCommand::Suspended) => paused = true,
                Step::Send(Signal::Activate) => assert!(paused),
                Step::Wait(Signal::Active) => active = true,
                Step::Ready | Step::Send(Signal::Commit) => assert!(active),
                Step::Wait(Signal::Committed) => committed = true,
                Step::Done => {
                    assert!(committed);
                    complete = true;
                }
                _ => {}
            }
            Ok(())
        })
        .unwrap();
        assert!(complete);
    }

    #[test]
    fn new_candidate_or_user_exit_cancels_before_commit() {
        let mut checks = 0;
        let mut commit = false;
        assert!(negotiate(|step| {
            if step == Step::CheckCurrent {
                checks += 1;
                if checks == 3 {
                    anyhow::bail!("new version or user exit");
                }
            }
            if step == Step::Send(Signal::Commit) {
                commit = true;
            }
            Ok(())
        })
        .is_err());
        assert!(!commit);
    }
}

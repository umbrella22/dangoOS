//! Bound continuous ready work independently of the duration of any one VM job.
// Pure yields retain the frequency work window; only real waits finish it.
const BUSY_BUDGET_US: u64 = 16_000;
const COOPERATIVE_BUDGET_US: u64 = 4_000;
const IDLE_POLL_US: u64 = 1_000;

#[cfg(target_os = "none")]
#[derive(Default)]
pub(super) struct RunProfile {
    pub turns: u64,
    pub entry_us: u64,
    pub work_us: u64,
    pub cooperate_us: u64,
    pub pause_us: u64,
    pub idle_us: u64,
    pub barrier_us: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Wait {
    Continue,
    Cooperate,
    BusyPause,
    Idle(u64),
}
#[derive(Default)]
pub(super) struct TurnBudget {
    busy_us: u64,
    cooperative_us: u64,
}
impl TurnBudget {
    pub fn record(&mut self, micros: u64) {
        self.busy_us = self.busy_us.saturating_add(micros);
        self.cooperative_us = self.cooperative_us.saturating_add(micros);
    }
    pub fn reset(&mut self) {
        self.busy_us = 0;
        self.cooperative_us = 0;
    }
    pub fn next_wait(&mut self, ready: bool, deadline: Option<u64>, now_us: u64) -> Wait {
        if ready || deadline.is_some_and(|end| end <= now_us) {
            if self.busy_us >= BUSY_BUDGET_US {
                self.reset();
                Wait::BusyPause
            } else if self.cooperative_us >= COOPERATIVE_BUDGET_US {
                self.cooperative_us = 0;
                Wait::Cooperate
            } else {
                Wait::Continue
            }
        } else {
            self.reset();
            Wait::Idle(
                deadline
                    .map_or(IDLE_POLL_US, |end| end - now_us)
                    .min(IDLE_POLL_US),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_engine::{CallState, HostReply, Limits, Package, Vm, GLOBAL_ALLOCATOR};

    #[test]
    fn completed_host_requests_resume_without_a_timer_wait() {
        let bytes =
            include_bytes!("../../../../artifacts/embedts/fixtures/scheduling-zero-sleep.etb");
        let mut vm = Vm::new(
            alloc::boxed::Box::new(()),
            Limits::default(),
            &GLOBAL_ALLOCATOR,
        )
        .unwrap();
        unsafe { vm.load(Package::parse(bytes).unwrap()) }.unwrap();
        assert_eq!(vm.start("main", None).unwrap(), CallState::Pending);
        let mut budget = TurnBudget::default();
        let mut completed = 0;
        let mut cooperations = 0;
        for _ in 0..300 {
            vm.drain_jobs(32).unwrap();
            while let Some(queued) = vm.take_request() {
                let request = match queued {
                    ts_engine::Request::Delay { id, .. } => {
                        vm.complete_void(id).unwrap();
                        continue;
                    }
                    ts_engine::Request::Host(request) => request,
                };
                vm.complete(request.id, Ok(HostReply::json("null")))
                    .unwrap();
                completed += 1;
            }
            if vm.entry_status().unwrap() == CallState::Done(42) {
                assert_eq!(completed, 256);
                assert!(cooperations >= 2);
                assert_eq!(vm.close().used, 0);
                return;
            }
            budget.record(50);
            match budget.next_wait(vm.has_pending_jobs(), None, 0) {
                Wait::Continue => {}
                Wait::Cooperate => cooperations += 1,
                other => panic!("unexpected timer wait: {other:?}"),
            }
        }
        panic!("entry did not finish");
    }
    #[test]
    fn continuous_ready_work_is_bounded_but_real_idle_resets_it() {
        let mut budget = TurnBudget::default();
        for turn in 1..=15 {
            budget.record(1_000);
            assert_eq!(
                budget.next_wait(true, None, 0),
                if turn % 4 == 0 {
                    Wait::Cooperate
                } else {
                    Wait::Continue
                }
            );
        }
        budget.record(1_000);
        assert_eq!(budget.next_wait(true, None, 0), Wait::BusyPause);
        assert_eq!(budget.next_wait(true, None, 0), Wait::Continue);
        budget.record(15_000);
        assert_eq!(budget.next_wait(false, Some(125), 100), Wait::Idle(25));
        budget.record(1_000);
        assert_eq!(budget.next_wait(false, Some(125), 125), Wait::Continue);
        assert_eq!(budget.next_wait(false, None, 125), Wait::Idle(1_000));
    }
}

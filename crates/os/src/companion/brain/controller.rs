//! Resident VM lifetime and isolated replacement across durable commits.
use super::{
    input::{Inbox, Input},
    BrainVm, Output,
};
use crate::companion::behavior::affect::SensorEvent;
use alloc::string::String;
use app_host::RawAllocator;

/// Resident brain ownership. Replacement validates init and first tick in an
/// isolated VM; no candidate output escapes before the swap.
pub struct Controller {
    active: Option<BrainVm>,
    source: String,
    allocator: &'static RawAllocator,
    clock: Option<fn() -> u64>,
    error: Option<String>,
    #[cfg(all(feature = "firmware", feature = "ts-engine"))]
    execution: crate::platform::runtime::frequency::device::Budget,
    #[cfg(all(feature = "firmware", feature = "embedscope"))]
    profile: CallbackProfile,
}

#[cfg(all(feature = "firmware", feature = "embedscope"))]
#[derive(Default)]
struct CallbackProfile {
    since: u64,
    calls: u32,
    total_us: u64,
    max_us: u64,
}
#[cfg(all(feature = "firmware", feature = "embedscope"))]
impl CallbackProfile {
    fn record(&mut self, elapsed: u64) {
        let now = crate::platform::runtime::time::now_us();
        self.calls += 1;
        self.total_us += elapsed;
        self.max_us = self.max_us.max(elapsed);
        if now.saturating_sub(self.since) >= 5_000_000 {
            log::info!(
                "brain: callbacks={} total_us={} max_us={} window_us={}",
                self.calls,
                self.total_us,
                self.max_us,
                now.saturating_sub(self.since)
            );
            *self = Self {
                since: now,
                ..Self::default()
            };
        }
    }
}
impl Controller {
    pub fn new(
        source: &str,
        now: u64,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
    ) -> Result<Self, String> {
        let active = BrainVm::new(source, now, allocator, clock)?;
        Ok(Self {
            active: Some(active),
            source: source.into(),
            allocator,
            clock,
            error: None,
            #[cfg(all(feature = "firmware", feature = "ts-engine"))]
            execution: crate::platform::runtime::frequency::device::Budget::new(2_000),
            #[cfg(all(feature = "firmware", feature = "embedscope"))]
            profile: CallbackProfile::default(),
        })
    }
    pub fn inactive(
        source: &str,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
        error: String,
    ) -> Self {
        Self {
            active: None,
            source: source.into(),
            allocator,
            clock,
            error: Some(error),
            #[cfg(all(feature = "firmware", feature = "ts-engine"))]
            execution: crate::platform::runtime::frequency::device::Budget::new(2_000),
            #[cfg(all(feature = "firmware", feature = "embedscope"))]
            profile: CallbackProfile::default(),
        }
    }
    pub fn replace(&mut self, source: &str, now: u64) -> Result<(), String> {
        self.replace_with_commit(source, now, || Ok(()))
    }
    pub fn replace_with_commit(
        &mut self,
        source: &str,
        now: u64,
        commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let candidate = BrainVm::new(source, now, self.allocator, self.clock)?;
        commit()?;
        self.active = Some(candidate);
        self.source = source.into();
        self.error = None;
        Ok(())
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Keep candidate VM/output private across an asynchronous durable commit.
    pub async fn replace_with_commit_async(
        &mut self,
        source: &str,
        now: u64,
        commit: impl AsyncFnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let candidate = BrainVm::new(source, now, self.allocator, self.clock)?;
        commit().await?;
        self.active = Some(candidate);
        self.source = source.into();
        self.error = None;
        Ok(())
    }
    pub fn active(&self) -> bool {
        self.active.is_some()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn output(&mut self) -> Option<Output> {
        self.active.as_mut().map(BrainVm::output)
    }
    pub fn dispatch(&mut self, input: String, now: u64) -> Option<String> {
        self.dispatch_inbox(Inbox::Raw(input), now)
    }
    fn dispatch_typed(&mut self, input: Input, now: u64) -> Option<String> {
        self.dispatch_inbox(Inbox::Typed(input), now)
    }
    fn dispatch_inbox(&mut self, input: Inbox, now: u64) -> Option<String> {
        #[cfg(all(feature = "firmware", feature = "embedscope"))]
        let started = crate::platform::runtime::time::now_us();
        let active = self.active.as_mut()?;
        #[cfg(all(feature = "firmware", feature = "ts-engine"))]
        let result = self.execution.run(true, || active.rpc_inbox(input, now));
        #[cfg(all(feature = "firmware", feature = "ts-engine"))]
        self.execution.finish();
        #[cfg(not(all(feature = "firmware", feature = "ts-engine")))]
        let result = active.rpc_inbox(input, now);
        #[cfg(all(feature = "firmware", feature = "embedscope"))]
        self.profile
            .record(crate::platform::runtime::time::now_us().saturating_sub(started));

        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.active = None;
                self.error = Some(e);
                // Report the transition once, after releasing the failed VM.
                // Later callbacks return above while inactive, without logging.
                log::warn!(
                    "brain: runtime fallback: {}",
                    crate::companion::brain::store::error_line(self.error.as_deref().unwrap_or(""))
                );
                None
            }
        }
    }
    pub fn tick(&mut self, now: u64) {
        self.dispatch_typed(Input::Tick(now), now);
    }
    pub fn event(&mut self, event: SensorEvent) {
        self.dispatch_typed(Input::Sensor(event), event.at_ms);
    }
    pub fn pointer(&mut self, p: crate::companion::behavior::personality::PointerSample) -> bool {
        self.dispatch_typed(Input::Pointer(p), p.at_ms)
            .is_some_and(|v| v != "false")
    }
    pub fn interaction(&mut self, now: u64) {
        self.dispatch_typed(Input::Interaction(now), now);
    }
    pub fn ignore_touch(&mut self, until: u64) {
        self.dispatch_typed(Input::IgnoreTouch(until), 0);
    }
}

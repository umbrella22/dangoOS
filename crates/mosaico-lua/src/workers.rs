//! Bounded cooperative worker group. Every job owns a distinct Lua state.
use crate::{Call, Error, Host, Limits, RawAllocator, Request, Ret, Step, Vm};
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;
use serde_json::{json, Value};
pub const REQUEST: u32 = 0x54480001;
const JOBS: usize = 8;
const HISTORY: usize = 32;
const SYNC: usize = 32;
const SYNC_BYTES: usize = 256 * 1024;
#[derive(Clone)]
pub(crate) struct Command {
    op: String,
    args: Value,
    bytes: Vec<u8>,
}
pub(crate) struct Reply {
    json: String,
    bytes: Vec<u8>,
}
fn values(v: Value) -> Reply {
    Reply {
        json: json!({"count":v.as_array().map_or(0,Vec::len),"values":v}).to_string(),
        bytes: Vec::new(),
    }
}
fn ok(s: &str) -> Reply {
    values(json!([true, s]))
}
fn fail(s: &str) -> Reply {
    values(json!([null, s]))
}
fn timed(op: &str, s: &str) -> Reply {
    if op == "queue_recv" {
        fail(s)
    } else {
        values(json!([false, s]))
    }
}
pub(crate) fn register(vm: &mut Vm) -> Result<(), String> {
    vm.register("_thread", &[("request", request), ("reply", reply)])
}
fn request(c: &mut Call) -> Result<Ret, Error> {
    if !c.can_yield() {
        return Err(Error::msg("thread requires a yieldable task"));
    }
    if c.ctx.bounded_worker_api() {
        return Err(Error::msg("thread unavailable in restricted callback VM"));
    }
    if c.bytes(2)?.len() > 65536 {
        return Err(Error::msg("thread arguments exceed 64 KiB"));
    }
    let op = c.str(1)?.into();
    let args = serde_json::from_str(c.str(2)?).map_err(|_| Error::msg("invalid thread command"))?;
    let bytes = c.bytes(3)?;
    if bytes.len() > 4096 {
        return Err(Error::msg("queue message exceeds 4096 bytes"));
    }
    c.ctx.worker_command = Some(Command {
        op,
        args,
        bytes: bytes.into(),
    });
    c.ctx.worker_waiting = true;
    Ok(Ret::Yield(Request::Host(REQUEST)))
}
fn reply(c: &mut Call) -> Result<Ret, Error> {
    let r = c
        .ctx
        .worker_reply
        .take()
        .ok_or_else(|| Error::msg("missing worker completion"))?;
    c.push_str(&r.json);
    c.push_bytes(&r.bytes);
    Ok(Ret::Values(2))
}
struct Logs(Rc<RefCell<String>>);
impl Host for Logs {
    fn print(&mut self, line: &[u8]) {
        let mut out = self.0.borrow_mut();
        let remain = 8192usize.saturating_sub(out.len());
        if remain > 0 {
            let line = String::from_utf8_lossy(line);
            for ch in line.chars() {
                if out.len() + ch.len_utf8() + 1 > 8192 {
                    break;
                }
                out.push(ch);
            }
            out.push('\n');
        }
    }
}
struct Queue {
    depth: usize,
    item: usize,
    messages: VecDeque<Vec<u8>>,
}
struct Semaphore {
    count: u8,
    max: u8,
}
struct Pending {
    command: Command,
    deadline: u64,
    run: Option<u64>,
}
struct Job {
    id: u64,
    name: String,
    exclusive: Option<String>,
    status: String,
    vm: Option<Vm>,
    logs: Rc<RefCell<String>>,
    error: String,
    wake: u64,
    deadline: Option<u64>,
    pending: Option<Pending>,
}
impl Job {
    fn summary(&self) -> String {
        format!(
            "id={} name={} status={}\n{}{}",
            self.id,
            self.name,
            self.status,
            self.error,
            self.logs.borrow()
        )
    }
}
pub(crate) struct Group {
    jobs: BTreeMap<u64, Job>,
    next: u64,
    queues: BTreeMap<String, Queue>,
    sems: BTreeMap<String, Semaphore>,
    locks: BTreeMap<String, Option<u64>>,
    parent: Option<Pending>,
    ticks: u64,
    limits: Limits,
    allocator: &'static RawAllocator,
    root: String,
}
impl Group {
    pub(crate) fn new(limits: Limits, allocator: &'static RawAllocator, root: String) -> Self {
        Self {
            jobs: BTreeMap::new(),
            next: 1,
            queues: BTreeMap::new(),
            sems: BTreeMap::new(),
            locks: BTreeMap::new(),
            parent: None,
            ticks: 0,
            limits,
            allocator,
            root,
        }
    }
    fn now(&mut self) -> u64 {
        self.ticks = self.ticks.saturating_add(1);
        self.limits.clock.map(|c| c() / 1000).unwrap_or(self.ticks)
    }
    fn complete(vm: &mut Vm, r: Reply) {
        unsafe {
            (*vm.ctx).worker_reply = Some(r);
            (*vm.ctx).worker_waiting = false;
        }
    }
    pub(crate) fn command(&mut self, vm: &mut Vm) {
        let cmd = unsafe { (*vm.ctx).worker_command.take() };
        if let Some(cmd) = cmd {
            let now = self.now();
            match self.dispatch(0, cmd.clone(), now) {
                Outcome::Reply(r) => Self::complete(vm, r),
                Outcome::Wait(p) => self.parent = Some(p),
            }
        }
    }
    pub(crate) fn tick(&mut self, parent: &mut Vm) {
        let now = self.now();
        if let Some(p) = self.parent.take() {
            match self.retry(0, p, now) {
                Outcome::Reply(r) => Self::complete(parent, r),
                Outcome::Wait(p) => self.parent = Some(p),
            }
        }
        let ids: Vec<_> = self.jobs.keys().copied().collect();
        for id in ids {
            let mut j = self.jobs.remove(&id).unwrap();
            if j.vm.is_none() {
                self.jobs.insert(id, j);
                continue;
            }
            if j.deadline.is_some_and(|d| now >= d) {
                j.status = "timeout".into();
                j.vm = None;
                j.pending = None;
                self.release_locks(id);
                self.jobs.insert(id, j);
                continue;
            }
            if let Some(p) = j.pending.take() {
                match self.retry(id, p, now) {
                    Outcome::Reply(r) => Self::complete(j.vm.as_mut().unwrap(), r),
                    Outcome::Wait(p) => {
                        j.pending = Some(p);
                        self.jobs.insert(id, j);
                        continue;
                    }
                }
            }
            if now < j.wake {
                self.jobs.insert(id, j);
                continue;
            }
            j.vm.as_mut().unwrap().set_callback_budget(1000, 500_000);
            let step = j.vm.as_mut().unwrap().resume_without_workers();
            match step {
                Step::Request(Request::Host(REQUEST)) => {
                    let cmd = unsafe { (*j.vm.as_ref().unwrap().ctx).worker_command.take() };
                    if let Some(cmd) = cmd {
                        match self.dispatch(id, cmd, now) {
                            Outcome::Reply(r) => Self::complete(j.vm.as_mut().unwrap(), r),
                            Outcome::Wait(p) => j.pending = Some(p),
                        }
                    }
                }
                Step::Request(Request::Sleep { ms }) => j.wake = now.saturating_add(ms as u64),
                Step::Done => {
                    j.status = "done".into();
                    j.vm = None;
                }
                Step::Interrupted => {
                    j.status = "stopped".into();
                    j.vm = None;
                }
                Step::Error(e) => {
                    j.status = "failed".into();
                    j.error = e.chars().take(2048).collect();
                    j.vm = None;
                }
                Step::Request(_) => {
                    j.status = "failed".into();
                    j.error = "worker requested unavailable UI".into();
                    j.vm = None;
                }
                Step::Slice => {}
            }
            if j.vm.is_none() {
                self.release_locks(id);
            }
            self.jobs.insert(id, j);
        }
    }
    fn stop_job(&mut self, id: u64) {
        let Some(mut job) = self.jobs.remove(&id) else {
            return;
        };
        if let Some(mut vm) = job.vm.take() {
            if let Some(pending) = job.pending.take() {
                Self::complete(&mut vm, timed(&pending.command.op, "stopped"));
                vm.set_callback_budget(1000, 500_000);
                // One bounded opportunity to observe the stopped wait. Further
                // requests are abandoned; arbitrary finalizers are never run.
                let _ = vm.resume_without_workers();
            }
            job.status = "stopped".into();
            drop(vm);
        }
        self.release_locks(id);
        self.jobs.insert(id, job);
    }
    fn release_locks(&mut self, id: u64) {
        for owner in self.locks.values_mut() {
            if *owner == Some(id) {
                *owner = None;
            }
        }
    }
    fn retry(&mut self, owner: u64, p: Pending, now: u64) -> Outcome {
        if let Some(id) = p.run {
            if let Some(j) = self.jobs.get(&id) {
                if j.vm.is_none() {
                    return Outcome::Reply(if j.status == "done" {
                        ok(&j.summary())
                    } else {
                        fail(&j.summary())
                    });
                }
            } else {
                return Outcome::Reply(fail("job not found"));
            }
            return Outcome::Wait(p);
        }
        let result = self.sync(owner, &p.command);
        match result {
            SyncResult::Reply(r) => Outcome::Reply(r),
            SyncResult::Blocked => {
                if now >= p.deadline {
                    Outcome::Reply(timed(&p.command.op, "timeout"))
                } else {
                    Outcome::Wait(p)
                }
            }
        }
    }
    fn find(&self, v: &Value) -> Option<u64> {
        v.as_u64()
            .filter(|id| self.jobs.contains_key(id))
            .or_else(|| {
                v.as_str().and_then(|name| {
                    self.jobs
                        .values()
                        .rev()
                        .find(|j| j.name == name)
                        .map(|j| j.id)
                })
            })
    }
    fn dispatch(&mut self, owner: u64, c: Command, now: u64) -> Outcome {
        let result = match c.op.as_str() {
            "start" | "run" => {
                if owner != 0 {
                    return Outcome::Reply(fail("nested workers are not supported"));
                }
                match self.start(&c.args, now) {
                    Ok(id) => {
                        if c.op == "run" {
                            return Outcome::Wait(Pending {
                                command: c,
                                deadline: u64::MAX,
                                run: Some(id),
                            });
                        }
                        ok(&self.jobs[&id].summary())
                    }
                    Err(e) => fail(&e),
                }
            }
            "get" => match self.find(&c.args[0]) {
                Some(id) => ok(&self.jobs[&id].summary()),
                None => fail("job not found"),
            },
            "list" => {
                let status = c.args[0].as_str().unwrap_or("all");
                if ![
                    "all", "queued", "running", "done", "failed", "timeout", "stopped",
                ]
                .contains(&status)
                {
                    fail("invalid job status")
                } else {
                    let mut out = String::new();
                    for j in self
                        .jobs
                        .values()
                        .filter(|j| status == "all" || j.status == status)
                    {
                        out.push_str(&j.summary());
                        out.push('\n');
                    }
                    ok(&out)
                }
            }
            "stop" => match self.find(&c.args[0]) {
                Some(id) => {
                    self.stop_job(id);
                    ok(&self.jobs[&id].summary())
                }
                None => fail("job not found"),
            },
            _ => match self.sync(owner, &c) {
                SyncResult::Reply(r) => r,
                SyncResult::Blocked => {
                    let index = 1;
                    let timeout = c.args[index].as_u64().unwrap_or(0).min(86_400_000);
                    if timeout == 0 {
                        timed(&c.op, "timeout")
                    } else {
                        return Outcome::Wait(Pending {
                            command: c,
                            deadline: now.saturating_add(timeout),
                            run: None,
                        });
                    }
                }
            },
        };
        Outcome::Reply(result)
    }
    fn start(&mut self, a: &Value, now: u64) -> Result<u64, String> {
        let path = a[0].as_str().ok_or("script path required")?;
        if !path.starts_with('/')
            || !path.ends_with(".lua")
            || path.split('/').any(|s| s == ".." || s == ".")
        {
            return Err("absolute .lua path required".into());
        }
        let opts = &a[2];
        let name = opts["name"]
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("job_{}", self.next));
        validate_name(&name)?;
        let exclusive = opts["exclusive"].as_str().map(ToString::to_string);
        let conflicting: Vec<_> = self
            .jobs
            .values()
            .filter(|j| {
                j.vm.is_some()
                    && (j.name == name
                        || exclusive
                            .as_ref()
                            .is_some_and(|e| j.exclusive.as_ref() == Some(e)))
            })
            .map(|j| j.id)
            .collect();
        if !conflicting.is_empty() && !opts["replace"].as_bool().unwrap_or(false) {
            return Err("job name or exclusive group already running".into());
        }
        for id in conflicting {
            self.stop_job(id);
        }
        if self.jobs.values().filter(|j| j.vm.is_some()).count() >= JOBS {
            return Err("worker limit reached".into());
        }
        while self.jobs.len() >= HISTORY {
            let old = self
                .jobs
                .iter()
                .find(|(_, j)| j.vm.is_none())
                .map(|(id, _)| *id)
                .ok_or("job history full")?;
            self.jobs.remove(&old);
        }
        let logs = Rc::new(RefCell::new(String::new()));
        let mut limits = self.limits;
        limits.memory_bytes = limits.memory_bytes.min(1024 * 1024);
        let mut vm = Vm::new(
            Box::new(Logs(Rc::clone(&logs))),
            limits,
            self.allocator,
            self.next as u32,
        )?;
        vm.disable_ui();
        vm.worker_child = true;
        vm.set_storage_root(&self.root)?;
        vm.set_args_json(&a[1].to_string())?;
        vm.load_file(path, &[])?;
        vm.restrict_worker()?;
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or("job identifier overflow")?;
        let timeout = opts["timeout_ms"].as_u64().unwrap_or(5000);
        let deadline = if timeout == 0 {
            None
        } else {
            Some(now.saturating_add(timeout))
        };
        self.jobs.insert(
            id,
            Job {
                id,
                name,
                exclusive,
                status: "running".into(),
                vm: Some(vm),
                logs,
                error: String::new(),
                wake: now,
                deadline,
                pending: None,
            },
        );
        Ok(id)
    }
    fn busy(&self, name: &str) -> bool {
        self.parent
            .iter()
            .chain(self.jobs.values().filter_map(|j| j.pending.as_ref()))
            .any(|p| p.command.args[0].as_str() == Some(name))
    }
    fn sync(&mut self, owner: u64, c: &Command) -> SyncResult {
        let name = match c.args[0].as_str() {
            Some(n) => n,
            None => return SyncResult::Reply(fail("sync name required")),
        };
        if let Err(e) = validate_name(name) {
            return SyncResult::Reply(fail(&e));
        }
        let a = &c.args;
        let family = c.op.split('_').next().unwrap_or("");
        let conflict = match family {
            "queue" => self.sems.contains_key(name) || self.locks.contains_key(name),
            "sem" => self.queues.contains_key(name) || self.locks.contains_key(name),
            _ => self.queues.contains_key(name) || self.sems.contains_key(name),
        };
        if conflict {
            return SyncResult::Reply(Reply {
                json: json!({"error":"sync name belongs to a different type"}).to_string(),
                bytes: Vec::new(),
            });
        }
        let result = match c.op.as_str() {
            "queue_create" => {
                let depth = a[1]["depth"].as_u64().unwrap_or(8).min(usize::MAX as u64) as usize;
                let item = a[1]["item_size"]
                    .as_u64()
                    .unwrap_or(256)
                    .min(usize::MAX as u64) as usize;
                if !(1..=32).contains(&depth) || !(1..=4096).contains(&item) {
                    fail("invalid queue bounds")
                } else if self.queues.contains_key(name) {
                    fail("exists")
                } else if self.queues.len() >= SYNC
                    || self
                        .queues
                        .values()
                        .map(|q| q.depth * q.item)
                        .sum::<usize>()
                        + depth * item
                        > SYNC_BYTES
                {
                    fail("queue registry budget exceeded")
                } else {
                    self.queues.insert(
                        name.into(),
                        Queue {
                            depth,
                            item,
                            messages: VecDeque::new(),
                        },
                    );
                    ok("created")
                }
            }
            "queue_send" => match self.queues.get_mut(name) {
                None => fail("not_found"),
                Some(q) => {
                    if c.bytes.len() > q.item {
                        fail("message exceeds item_size")
                    } else if q.messages.len() >= q.depth {
                        return SyncResult::Blocked;
                    } else {
                        q.messages.push_back(c.bytes.clone());
                        ok("sent")
                    }
                }
            },
            "queue_recv" => match self.queues.get_mut(name) {
                None => fail("not_found"),
                Some(q) => {
                    if let Some(bytes) = q.messages.pop_front() {
                        Reply {
                            json: json!({"binary":true}).to_string(),
                            bytes,
                        }
                    } else {
                        return SyncResult::Blocked;
                    }
                }
            },
            "queue_delete" => {
                if self.busy(name)
                    || self
                        .queues
                        .get(name)
                        .is_some_and(|q| !q.messages.is_empty())
                {
                    fail("busy")
                } else if self.queues.remove(name).is_some() {
                    ok("deleted")
                } else {
                    fail("not_found")
                }
            }
            "sem_create" => {
                let max = a[1]["max"].as_u64().unwrap_or(1);
                let initial = a[1]["initial"].as_u64().unwrap_or(0);
                if max < 1 || max > 255 || initial > max {
                    fail("invalid semaphore bounds")
                } else if self.sems.contains_key(name) {
                    fail("exists")
                } else if self.sems.len() >= SYNC {
                    fail("semaphore limit reached")
                } else {
                    self.sems.insert(
                        name.into(),
                        Semaphore {
                            count: initial as u8,
                            max: max as u8,
                        },
                    );
                    ok("created")
                }
            }
            "sem_take" => match self.sems.get_mut(name) {
                None => fail("not_found"),
                Some(s) => {
                    if s.count == 0 {
                        return SyncResult::Blocked;
                    } else {
                        s.count -= 1;
                        ok("taken")
                    }
                }
            },
            "sem_give" => match self.sems.get_mut(name) {
                None => fail("not_found"),
                Some(s) => {
                    if s.count == s.max {
                        values(json!([false, "full"]))
                    } else {
                        s.count += 1;
                        ok("given")
                    }
                }
            },
            "sem_delete" => {
                if self.busy(name) {
                    fail("busy")
                } else if self.sems.remove(name).is_some() {
                    ok("deleted")
                } else {
                    fail("not_found")
                }
            }
            "lock_create" => {
                if self.locks.contains_key(name) {
                    fail("exists")
                } else if self.locks.len() >= SYNC {
                    fail("lock limit reached")
                } else {
                    self.locks.insert(name.into(), None);
                    ok("created")
                }
            }
            "lock" => match self.locks.get_mut(name) {
                None => fail("not_found"),
                Some(slot) => {
                    if slot.is_some() {
                        return SyncResult::Blocked;
                    } else {
                        *slot = Some(owner);
                        ok("locked")
                    }
                }
            },
            "unlock" => match self.locks.get_mut(name) {
                None => fail("not_found"),
                Some(slot) => {
                    if *slot != Some(owner) {
                        values(json!([false, "not_owner"]))
                    } else {
                        *slot = None;
                        ok("unlocked")
                    }
                }
            },
            "lock_delete" => {
                if self.busy(name) || self.locks.get(name).is_some_and(Option::is_some) {
                    fail("busy")
                } else if self.locks.remove(name).is_some() {
                    ok("deleted")
                } else {
                    fail("not_found")
                }
            }
            _ => fail("unknown thread operation"),
        };
        SyncResult::Reply(result)
    }
}
enum Outcome {
    Reply(Reply),
    Wait(Pending),
}
enum SyncResult {
    Reply(Reply),
    Blocked,
}
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 31 {
        Err("name length must be 1..31".into())
    } else if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        Err("invalid character in sync name".into())
    } else {
        Ok(())
    }
}
impl Vm {
    /// Progresses independent workers without resuming a sleeping foreground VM.
    pub fn poll_workers(&mut self) {
        if let Some(mut group) = self.workers.take() {
            group.tick(self);
            self.workers = Some(group);
        }
    }
    pub(crate) fn handle_worker_command(&mut self) {
        let mut group = self.workers.take().unwrap_or_else(|| {
            let (limits, allocator, root) = self.worker_settings();
            Box::new(Group::new(limits, allocator, root))
        });
        group.command(self);
        self.workers = Some(group);
    }
}

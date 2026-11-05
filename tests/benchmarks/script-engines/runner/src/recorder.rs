use serde_json::{json, Value};
use std::time::Instant;

struct Active {
    case: String,
    sample: u64,
    wall: Instant,
    segment: Option<Instant>,
    vm_ns: u64,
}

#[derive(Default)]
pub struct Recorder {
    pub events: Vec<Value>,
    pub error: Option<String>,
    active: Option<Active>,
    entered: Option<Instant>,
    pub max_vm_step_ns: u64,
    sequence: u64,
    fragments: Vec<String>,
    fragment_total: usize,
}

impl Recorder {
    pub fn enter(&mut self) {
        let now = Instant::now();
        self.entered = Some(now);
        if let Some(active) = &mut self.active {
            active.segment = Some(now);
        }
    }
    pub fn leave(&mut self) {
        let now = Instant::now();
        if let Some(entered) = self.entered.take() {
            self.max_vm_step_ns = self.max_vm_step_ns.max((now - entered).as_nanos() as u64);
        }
        if let Some(active) = &mut self.active {
            if let Some(start) = active.segment.take() {
                active.vm_ns += (now - start).as_nanos() as u64;
            }
        }
    }
    pub fn line(&mut self, line: &str) {
        if let Err(error) = self.record(line) {
            self.error = Some(error);
        }
    }
    fn record(&mut self, line: &str) -> Result<(), String> {
        if let Some(raw) = line.strip_prefix("DANGO_BENCH_PART ") {
            let (header, content) = raw.split_once(' ').ok_or("invalid fragment header")?;
            let numbers: Vec<u64> = header
                .split('/')
                .map(|n| n.parse::<u64>())
                .collect::<Result<_, _>>()
                .map_err(|_| "invalid fragment header")?;
            if numbers.len() != 3 {
                return Err("invalid fragment header".into());
            }
            let (sequence, index, total) = (numbers[0], numbers[1] as usize, numbers[2] as usize);
            if sequence != self.sequence + 1
                || index != self.fragments.len() + 1
                || !(1..=64).contains(&total)
                || index > total
                || (!self.fragments.is_empty() && total != self.fragment_total)
            {
                return Err("missing, duplicate or out-of-order fragment".into());
            }
            if !content.is_ascii()
                || content.is_empty()
                || content.len() > 64
                || (index < total && content.len() != 64)
            {
                return Err("truncated or invalid fragment".into());
            }
            self.fragments.push(content.into());
            self.fragment_total = total;
            if index == total {
                let raw = self.fragments.concat();
                self.fragments.clear();
                self.sequence = sequence;
                return self.event(&raw);
            }
            return Ok(());
        }
        if !self.fragments.is_empty() {
            return Err("incomplete fragment".into());
        }
        let raw = line
            .strip_prefix("DANGO_BENCH ")
            .ok_or("unexpected script output")?;
        self.event(raw)
    }
    fn event(&mut self, raw: &str) -> Result<(), String> {
        let now = Instant::now();
        let mut event: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        match event["event"].as_str() {
            Some("begin") => {
                if self.active.is_some() {
                    return Err("overlapping benchmark samples".into());
                }
                self.active = Some(Active {
                    case: event["case"].as_str().ok_or("missing case")?.into(),
                    sample: event["sample"].as_u64().ok_or("missing sample")?,
                    wall: now,
                    segment: self.entered.map(|_| now),
                    vm_ns: 0,
                });
            }
            Some("sample") => {
                let mut active = self.active.take().ok_or("sample without begin")?;
                if event["case"] != active.case || event["sample"] != active.sample {
                    return Err("sample identity mismatch".into());
                }
                if let Some(start) = active.segment.take() {
                    active.vm_ns += (now - start).as_nanos() as u64;
                }
                event["host_vm_ns"] = json!(active.vm_ns);
                event["host_wall_ns"] = json!((now - active.wall).as_nanos() as u64);
            }
            Some("start" | "warmup" | "done") => {}
            _ => return Err("unknown benchmark event".into()),
        }
        self.events.push(event);
        Ok(())
    }
    pub fn finish(&self) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.active.is_some()
            || !self.fragments.is_empty()
            || self.events.last().map(|v| v["event"].as_str()) != Some(Some("done"))
        {
            return Err("incomplete benchmark".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_time_is_excluded_from_vm_time() {
        let mut r = Recorder::default();
        r.enter();
        r.line(r#"DANGO_BENCH {"event":"begin","case":"integer","sample":0}"#);
        r.leave();
        std::thread::sleep(std::time::Duration::from_millis(5));
        r.enter();
        r.line(r#"DANGO_BENCH {"event":"sample","case":"integer","sample":0}"#);
        r.leave();
        r.line(r#"DANGO_BENCH {"event":"done"}"#);
        r.finish().unwrap();
        let sample = &r.events[1];
        assert!(
            sample["host_wall_ns"].as_u64().unwrap() - sample["host_vm_ns"].as_u64().unwrap()
                >= 4_000_000
        );
    }

    #[test]
    fn partial_and_mismatched_samples_fail() {
        let mut r = Recorder::default();
        r.line(r#"DANGO_BENCH {"event":"begin","case":"sort","sample":0}"#);
        assert!(r.finish().is_err());
        r.line(r#"DANGO_BENCH {"event":"sample","case":"sort","sample":1}"#);
        assert!(r.finish().is_err());
    }

    #[test]
    fn framed_events_reassemble_and_detect_lost_fragments() {
        let event = serde_json::json!({"event":"start","padding":"x".repeat(150)}).to_string();
        let chunks: Vec<_> = event.as_bytes().chunks(64).collect();
        let mut r = Recorder::default();
        for (index, chunk) in chunks.iter().enumerate() {
            r.line(&format!(
                "DANGO_BENCH_PART 1/{}/{} {}",
                index + 1,
                chunks.len(),
                std::str::from_utf8(chunk).unwrap()
            ));
        }
        r.line("DANGO_BENCH_PART 2/1/1 {\"event\":\"done\"}");
        r.finish().unwrap();
        assert_eq!(r.events.len(), 2);
        let mut broken = Recorder::default();
        broken.line(&format!("DANGO_BENCH_PART 1/1/3 {}", "x".repeat(64)));
        broken.line("DANGO_BENCH_PART 1/3/3 lost");
        assert!(broken.finish().is_err());
    }
}

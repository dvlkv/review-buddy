//! Simple in-process timers and counters, read by the debug HUD and perf runs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

type Samples = (HashMap<String, Vec<f64>>, HashMap<String, usize>);

#[derive(Default)]
pub struct Perf {
    inner: Mutex<Samples>,
}

impl Perf {
    pub fn shared() -> &'static Perf {
        static P: OnceLock<Perf> = OnceLock::new();
        P.get_or_init(Perf::default)
    }

    pub fn count(&self, name: &str, n: usize) {
        *self.inner.lock().unwrap().1.entry(name.into()).or_default() += n;
    }

    pub fn counter(&self, name: &str) -> usize {
        self.inner.lock().unwrap().1.get(name).copied().unwrap_or(0)
    }

    pub fn measure<T>(&self, name: &str, body: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let r = body();
        self.record(name, start.elapsed().as_secs_f64() * 1000.0);
        r
    }

    pub fn record(&self, name: &str, ms: f64) {
        let mut g = self.inner.lock().unwrap();
        let s = g.0.entry(name.into()).or_default();
        s.push(ms);
        if s.len() > 500 {
            s.drain(0..250);
        }
    }

    pub fn last(&self, name: &str) -> Option<f64> {
        self.inner.lock().unwrap().0.get(name).and_then(|s| s.last().copied())
    }

    pub fn percentile(&self, name: &str, p: f64) -> Option<f64> {
        let g = self.inner.lock().unwrap();
        let s = g.0.get(name)?;
        if s.is_empty() {
            return None;
        }
        let mut sorted = s.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)])
    }

    pub fn report(&self) -> String {
        let g = self.inner.lock().unwrap();
        let mut lines = Vec::new();
        let mut ck: Vec<_> = g.1.keys().collect();
        ck.sort();
        for k in ck {
            lines.push(format!("{} count={}", k, g.1[k]));
        }
        let mut sk: Vec<_> = g.0.keys().collect();
        sk.sort();
        for k in sk {
            let mut s = g.0[k].clone();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p50 = s[s.len() / 2];
            let p95 = s[((s.len() as f64 * 0.95) as usize).min(s.len() - 1)];
            lines.push(format!("{} n={} p50={:.2}ms p95={:.2}ms", k, s.len(), p50, p95));
        }
        lines.join("\n")
    }

    pub fn reset(&self) {
        let mut g = self.inner.lock().unwrap();
        g.0.clear();
        g.1.clear();
    }
}

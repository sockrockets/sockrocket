//! Bounded, per-server AnyTLS padding policies; no global cross-node state.
use anyhow::{Result, ensure};
use rand::Rng;
use std::collections::BTreeMap;
const DEFAULT: &str = "stop=8\n0=30-30\n1=100-400\n2=400-500,c,500-1000,c,500-1000,c,500-1000,c,500-1000\n3=9-9,500-1000\n4=500-1000\n5=500-1000\n6=500-1000\n7=500-1000";
#[derive(Clone)]
pub(super) struct Padding {
    stop: u32,
    rules: BTreeMap<u32, Vec<Option<(usize, usize)>>>,
    pub md5: String,
}
impl Default for Padding {
    fn default() -> Self {
        Self::parse(DEFAULT.as_bytes()).expect("built-in AnyTLS padding")
    }
}
impl Padding {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        use md5::{Digest, Md5};
        ensure!(raw.len() <= 8192, "AnyTLS padding policy too large");
        let text = std::str::from_utf8(raw)?;
        let mut stop = None;
        let mut rules = BTreeMap::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let (key, value) = line
                .trim()
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("Invalid AnyTLS padding policy"))?;
            if key == "stop" {
                ensure!(stop.is_none(), "Duplicate padding stop");
                stop = Some(value.parse::<u32>()?);
                continue;
            }
            let index = key.parse::<u32>()?;
            ensure!(
                index < 128 && !rules.contains_key(&index),
                "Invalid padding packet index"
            );
            let mut ranges = Vec::new();
            let mut budget = 0;
            for part in value.split(',') {
                ensure!(ranges.len() < 16, "Too many padding segments");
                if part == "c" {
                    ranges.push(None);
                    continue;
                }
                let (lo, hi) = part
                    .split_once('-')
                    .ok_or_else(|| anyhow::anyhow!("Invalid padding range"))?;
                let lo = lo.parse::<usize>()?;
                let hi = hi.parse::<usize>()?;
                ensure!(lo > 0 && lo <= hi && hi <= 16384, "Invalid padding size");
                budget += hi;
                ensure!(budget <= 65536, "Padding record budget exceeded");
                ranges.push(Some((lo, hi)));
            }
            rules.insert(index, ranges);
        }
        let stop = stop.ok_or_else(|| anyhow::anyhow!("Missing padding stop"))?;
        ensure!(stop <= 128, "Padding stop exceeds limit");
        Ok(Self {
            stop,
            rules,
            md5: format!("{:x}", Md5::digest(raw)),
        })
    }
    pub fn sizes(&self, packet: u32) -> Vec<Option<usize>> {
        if packet >= self.stop {
            return vec![];
        }
        self.rules
            .get(&packet)
            .into_iter()
            .flatten()
            .map(|range| range.map(|(lo, hi)| rand::rng().random_range(lo..=hi)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_policy_cannot_allocate_unbounded_records() {
        for raw in [
            "stop=4294967295",
            "stop=8\n1=1-99999999",
            "stop=8\n1=30-10",
            "stop=8\n1=1-2\n1=3-4",
            "stop=8\n1=16384-16384,16384-16384,16384-16384,16384-16384,16384-16384",
        ] {
            assert!(Padding::parse(raw.as_bytes()).is_err());
        }
        assert!(Padding::parse(&vec![b'x'; 8193]).is_err());
        let defaults = Padding::default();
        assert!(defaults.sizes(8).is_empty());
        assert_eq!(defaults.sizes(0), vec![Some(30)]);
    }
}

//! LRU cache for rendered page bitmaps.
//!
//! Keyed by `(page index, quantized scale)`, bounded by a byte budget.
//! Quantizing the scale to integer millis prevents float-keyed cache misses
//! and stops near-identical zoom levels from piling up distinct entries.

use std::collections::HashMap;

use syodep_pdf::Bitmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    page: usize,
    scale_milli: u32,
}

fn quantize(scale: f32) -> u32 {
    (scale * 1000.0).round().max(1.0) as u32
}

#[derive(Debug)]
struct Entry {
    bitmap: Bitmap,
    last_used: u64,
}

/// Byte-bounded LRU cache of rendered pages.
#[derive(Debug)]
pub struct RenderCache {
    entries: HashMap<CacheKey, Entry>,
    budget_bytes: usize,
    used_bytes: usize,
    tick: u64,
}

impl RenderCache {
    pub const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget_bytes,
            used_bytes: 0,
            tick: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Drop everything (e.g. when a new document is opened).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.used_bytes = 0;
    }

    /// Fetch a bitmap, rendering it with `render` on a miss.
    pub fn get_or_render<E>(
        &mut self,
        page: usize,
        scale: f32,
        render: impl FnOnce() -> Result<Bitmap, E>,
    ) -> Result<&Bitmap, E> {
        let key = CacheKey {
            page,
            scale_milli: quantize(scale),
        };
        self.tick += 1;
        if !self.entries.contains_key(&key) {
            let bitmap = render()?;
            self.used_bytes += bitmap.data.len();
            self.entries.insert(
                key,
                Entry {
                    bitmap,
                    last_used: self.tick,
                },
            );
            self.evict_over_budget(key);
        }
        let tick = self.tick;
        let entry = self
            .entries
            .get_mut(&key)
            .expect("just inserted or present");
        entry.last_used = tick;
        Ok(&entry.bitmap)
    }

    /// Evict least-recently-used entries until within budget, never evicting
    /// `keep` (the entry that was just inserted, which may alone exceed the
    /// budget for huge pages).
    fn evict_over_budget(&mut self, keep: CacheKey) {
        while self.used_bytes > self.budget_bytes && self.entries.len() > 1 {
            let Some((&oldest, _)) = self
                .entries
                .iter()
                .filter(|(k, _)| **k != keep)
                .min_by_key(|(_, e)| e.last_used)
            else {
                break;
            };
            let removed = self.entries.remove(&oldest).expect("key from iteration");
            self.used_bytes -= removed.bitmap.data.len();
        }
    }
}

impl Default for RenderCache {
    fn default() -> Self {
        Self::new(Self::DEFAULT_BUDGET_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(bytes: usize) -> Bitmap {
        Bitmap {
            width: bytes as u32 / 4,
            height: 1,
            data: vec![0; bytes],
        }
    }

    #[test]
    fn caches_renders() {
        let mut cache = RenderCache::new(1024);
        let mut renders = 0;
        for _ in 0..3 {
            cache
                .get_or_render(0, 1.0, || -> Result<_, ()> {
                    renders += 1;
                    Ok(bitmap(100))
                })
                .unwrap();
        }
        assert_eq!(renders, 1);
        assert_eq!(cache.used_bytes(), 100);
    }

    #[test]
    fn different_scales_are_different_entries() {
        let mut cache = RenderCache::new(1024);
        cache
            .get_or_render(0, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        cache
            .get_or_render(0, 2.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn near_identical_scales_share_an_entry() {
        let mut cache = RenderCache::new(1024);
        cache
            .get_or_render(0, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        cache
            .get_or_render(0, 1.0001, || -> Result<_, ()> {
                panic!("should be a cache hit")
            })
            .unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn evicts_least_recently_used_when_over_budget() {
        let mut cache = RenderCache::new(250);
        cache
            .get_or_render(0, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        cache
            .get_or_render(1, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        // Touch page 0 so page 1 is the LRU.
        cache
            .get_or_render(0, 1.0, || -> Result<_, ()> {
                panic!("should be a cache hit")
            })
            .unwrap();
        cache
            .get_or_render(2, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        assert_eq!(cache.len(), 2);
        assert!(cache.used_bytes() <= 250);
        // Page 1 was evicted, pages 0 and 2 remain.
        let mut rerendered = false;
        cache
            .get_or_render(1, 1.0, || -> Result<_, ()> {
                rerendered = true;
                Ok(bitmap(100))
            })
            .unwrap();
        assert!(rerendered);
    }

    #[test]
    fn single_oversized_entry_is_kept() {
        let mut cache = RenderCache::new(50);
        cache
            .get_or_render(0, 1.0, || -> Result<_, ()> { Ok(bitmap(100)) })
            .unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn render_errors_propagate_and_do_not_poison() {
        let mut cache = RenderCache::new(1024);
        let err = cache.get_or_render(0, 1.0, || Err("boom"));
        assert_eq!(err.unwrap_err(), "boom");
        cache
            .get_or_render(0, 1.0, || -> Result<_, &str> { Ok(bitmap(100)) })
            .unwrap();
        assert_eq!(cache.len(), 1);
    }
}

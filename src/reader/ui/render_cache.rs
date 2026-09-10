use cairo::ImageSurface;

/// レンダー済みページの置き場。キーは (ページ番号, ズーム倍率)。
/// 上限を超えたら最も古いものから捨てる (ヒットすると新しくなる)
pub struct PageRenderCache {
    max_pages: usize,
    entries: Vec<(usize, f64, ImageSurface)>,
}

impl PageRenderCache {
    pub fn new(max_pages: usize) -> Self {
        Self {
            max_pages,
            entries: Vec::new(),
        }
    }

    pub fn get(&mut self, index: usize, zoom: f64) -> Option<&ImageSurface> {
        let pos = self
            .entries
            .iter()
            .position(|(i, z, _)| *i == index && *z == zoom)?;
        let entry = self.entries.remove(pos);
        self.entries.push(entry);
        Some(&self.entries.last().expect("入れたばかりの要素").2)
    }

    pub fn insert(&mut self, index: usize, zoom: f64, surface: ImageSurface) {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|(i, z, _)| *i == index && *z == zoom)
        {
            self.entries[pos].2 = surface;
            return;
        }
        self.entries.push((index, zoom, surface));
        while self.entries.len() > self.max_pages {
            self.entries.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(w: i32, h: i32) -> ImageSurface {
        ImageSurface::create(cairo::Format::Rgb24, w, h).expect("surface を作れること")
    }

    fn dims(s: &ImageSurface) -> (i32, i32) {
        (s.width(), s.height())
    }

    #[test]
    fn get_returns_none_until_inserted() {
        let mut c = PageRenderCache::new(2);
        assert!(c.get(0, 1.0).is_none());
        assert!(c.get(5, 1.5).is_none());
    }

    #[test]
    fn get_returns_the_inserted_surface() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        assert_eq!(dims(c.get(0, 1.0).expect("あるはず")), (10, 10));
    }

    #[test]
    fn zoom_is_part_of_the_key() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        assert!(c.get(0, 1.5).is_none(), "同じページでも倍率が違えば別物");
        assert!(c.get(1, 1.0).is_none(), "同じ倍率でもページが違えば別物");
    }

    #[test]
    fn inserting_the_same_key_replaces_the_surface() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        c.insert(0, 1.0, surface(20, 20));
        assert_eq!(dims(c.get(0, 1.0).expect("あるはず")), (20, 20));
    }

    #[test]
    fn evicts_the_oldest_when_over_cap() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        c.insert(1, 1.0, surface(20, 20));
        c.insert(2, 1.0, surface(30, 30));
        assert!(c.get(0, 1.0).is_none(), "最初に入れたものが追い出される");
        assert!(c.get(1, 1.0).is_some());
        assert!(c.get(2, 1.0).is_some());
    }

    #[test]
    fn a_hit_makes_the_entry_fresh() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        c.insert(1, 1.0, surface(20, 20));
        c.get(0, 1.0).expect("あるはず");
        c.insert(2, 1.0, surface(30, 30));
        assert!(c.get(1, 1.0).is_none(), "触らなかったものが追い出される");
        assert!(c.get(0, 1.0).is_some(), "触ったものは残る");
    }

    #[test]
    fn clear_empties_the_cache() {
        let mut c = PageRenderCache::new(2);
        c.insert(0, 1.0, surface(10, 10));
        c.clear();
        assert!(c.get(0, 1.0).is_none());
    }
}

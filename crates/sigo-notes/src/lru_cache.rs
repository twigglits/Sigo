use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

pub struct LruCache<K, V> {
    inner: Mutex<Inner<K, V>>,
}

struct Inner<K, V> {
    capacity: usize,
    map: HashMap<K, usize>,
    nodes: Vec<Option<Node<K, V>>>,
    free: Vec<usize>,
    head: Option<usize>,
    tail: Option<usize>,
    len: usize,
}

struct Node<K, V> {
    key: K,
    value: V,
    prev: Option<usize>,
    next: Option<usize>,
}

impl<K: Eq + Hash + Clone, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        LruCache {
            inner: Mutex::new(Inner {
                capacity,
                map: HashMap::new(),
                nodes: Vec::new(),
                free: Vec::new(),
                head: None,
                tail: None,
                len: 0,
            }),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let mut g = self.inner.lock().unwrap();
        g.get(key)
    }

    pub fn insert(&self, key: K, value: V) {
        let mut g = self.inner.lock().unwrap();
        g.insert(key, value);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Inner<K, V> {
    fn detach(&mut self, idx: usize) {
        let (prev, next) = {
            let n = self.nodes[idx].as_ref().unwrap();
            (n.prev, n.next)
        };
        match prev {
            Some(p) => self.nodes[p].as_mut().unwrap().next = next,
            None => self.head = next,
        }
        match next {
            Some(n) => self.nodes[n].as_mut().unwrap().prev = prev,
            None => self.tail = prev,
        }
        let n = self.nodes[idx].as_mut().unwrap();
        n.prev = None;
        n.next = None;
    }

    fn push_front(&mut self, idx: usize) {
        let old_head = self.head;
        self.nodes[idx].as_mut().unwrap().prev = None;
        self.nodes[idx].as_mut().unwrap().next = old_head;
        match old_head {
            Some(h) => self.nodes[h].as_mut().unwrap().prev = Some(idx),
            None => self.tail = Some(idx),
        }
        self.head = Some(idx);
    }

    fn alloc_slot(&mut self, key: K, value: V) -> usize {
        let node = Node { key, value, prev: None, next: None };
        if let Some(idx) = self.free.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            self.nodes.push(Some(node));
            self.nodes.len() - 1
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let idx = *self.map.get(key)?;
        self.detach(idx);
        self.push_front(idx);
        Some(self.nodes[idx].as_ref().unwrap().value.clone())
    }

    fn insert(&mut self, key: K, value: V) {
        if let Some(&idx) = self.map.get(&key) {
            self.nodes[idx].as_mut().unwrap().value = value;
            self.detach(idx);
            self.push_front(idx);
            return;
        }

        let idx = if self.len == self.capacity {
            let evict = self.tail.expect("tail must exist when len == capacity");
            let evict_key = self.nodes[evict].as_ref().unwrap().key.clone();
            self.detach(evict);
            self.map.remove(&evict_key);
            self.nodes[evict] = Some(Node { key: key.clone(), value, prev: None, next: None });
            evict
        } else {
            self.len += 1;
            self.alloc_slot(key.clone(), value)
        };

        self.map.insert(key, idx);
        self.push_front(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_none_on_empty_cache() {
        let cache: LruCache<i32, &str> = LruCache::new(2);
        assert_eq!(cache.get(&1), None);
    }

    #[test]
    fn insert_and_get_single_item() {
        let cache = LruCache::new(2);
        cache.insert(1, "one");
        assert_eq!(cache.get(&1), Some("one"));
    }

    #[test]
    fn evicts_least_recently_used_when_at_capacity() {
        let cache = LruCache::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        cache.insert(3, "three"); // evicts 1 (LRU)
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some("two"));
        assert_eq!(cache.get(&3), Some("three"));
    }

    #[test]
    fn get_promotes_item_preventing_its_eviction() {
        let cache = LruCache::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        cache.get(&1); // promote 1 → 2 becomes LRU
        cache.insert(3, "three"); // evicts 2
        assert_eq!(cache.get(&1), Some("one"));
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&3), Some("three"));
    }

    #[test]
    fn insert_overwrites_value_for_existing_key() {
        let cache = LruCache::new(2);
        cache.insert(1, "one");
        cache.insert(1, "ONE");
        assert_eq!(cache.get(&1), Some("ONE"));
    }

    #[test]
    fn insert_update_promotes_key_preventing_eviction() {
        let cache = LruCache::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        cache.insert(1, "ONE"); // re-insert 1 → 2 becomes LRU
        cache.insert(3, "three"); // evicts 2
        assert_eq!(cache.get(&1), Some("ONE"));
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&3), Some("three"));
    }

    #[test]
    fn capacity_one_evicts_on_second_insert() {
        let cache = LruCache::new(1);
        cache.insert(1, "one");
        cache.insert(2, "two");
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some("two"));
    }

    #[test]
    fn len_tracks_entry_count_up_to_capacity() {
        let cache = LruCache::new(3);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        cache.insert(1, "one");
        assert_eq!(cache.len(), 1);
        cache.insert(2, "two");
        assert_eq!(cache.len(), 2);
        cache.insert(3, "three");
        assert_eq!(cache.len(), 3);
        cache.insert(4, "four"); // evicts, len stays 3
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn eviction_order_across_multiple_inserts() {
        let cache = LruCache::new(3);
        cache.insert(1, 'a');
        cache.insert(2, 'b');
        cache.insert(3, 'c');
        cache.get(&1); // order: 1(MRU) 3 2(LRU)
        cache.get(&3); // order: 3(MRU) 1 2(LRU)
        cache.insert(4, 'd'); // evicts 2
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&1), Some('a'));
        assert_eq!(cache.get(&3), Some('c'));
        assert_eq!(cache.get(&4), Some('d'));
    }

    #[test]
    fn thread_safety_concurrent_inserts() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(LruCache::new(100));
        let handles: Vec<_> = (0..10)
            .map(|t| {
                let c = Arc::clone(&cache);
                thread::spawn(move || {
                    for i in 0..10usize {
                        c.insert(t * 10 + i, t * 10 + i);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cache.len(), 100);
    }
}

use std::collections::HashMap;
use std::ops::Range;

#[derive(Debug)]
pub struct StreamJoinState<K, BufV, StreamV> {
    cur_stream_val: Option<(K, StreamV)>,
    // TODO(justin): we could maybe do something fancier here, like keep an
    // index into the set of values since we know we won't have anyone else try
    // to read or write it while we're emitting values, but keep it simple for
    // now.
    buf: Vec<BufV>,
    tab: HashMap<K, Vec<BufV>>,
}

impl<K, BufV, StreamV> Default for StreamJoinState<K, BufV, StreamV> {
    fn default() -> Self {
        Self {
            cur_stream_val: None,
            buf: Vec::new(),
            tab: HashMap::new(),
        }
    }
}

pub struct StreamJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    buf: Buf,
    stream: Stream,
    state: &'a mut StreamJoinState<K, BufV, StreamV>,
}

impl<'a, K, Buf, BufV, Stream, StreamV> Iterator for StreamJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash + Clone,
    BufV: Clone,
    StreamV: Clone,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    type Item = (K, StreamV, BufV);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(buf_v) = self.state.buf.pop() {
                let (k, stream_v) = self.state.cur_stream_val.clone().unwrap();
                return Some((k, stream_v, buf_v));
            }

            // TODO(justin): reuse this last value.
            self.state.cur_stream_val = None;

            for (k, v) in &mut self.buf {
                self.state.tab.entry(k).or_insert_with(Vec::new).push(v);
            }

            for (k, v) in &mut self.stream {
                if let Some(vals) = self.state.tab.get(&k) {
                    self.state.cur_stream_val = Some((k, v));
                    self.state.buf = vals.clone();
                    break;
                }
            }
            self.state.cur_stream_val.as_ref()?;
        }
    }
}
impl<'a, K, Buf, BufV, Stream, StreamV> StreamJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    pub fn new(buf: Buf, stream: Stream, state: &'a mut StreamJoinState<K, BufV, StreamV>) -> Self {
        Self { buf, stream, state }
    }
}

#[derive(Debug)]
pub struct BatchJoinState<K, BufV> {
    tab: HashMap<K, Vec<BufV>>,
}

impl<K, BufV> Default for BatchJoinState<K, BufV> {
    fn default() -> Self {
        Self {
            tab: HashMap::new(),
        }
    }
}

pub struct BatchJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    buf: Buf,
    stream: Stream,
    state: &'a mut BatchJoinState<K, BufV>,
}

impl<'a, K, Buf, BufV, Stream, StreamV> Iterator for BatchJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    type Item = (K, StreamV, Vec<BufV>);

    fn next(&mut self) -> Option<Self::Item> {
        for (k, v) in &mut self.buf {
            self.state.tab.entry(k).or_insert_with(Vec::new).push(v);
        }

        for (k, v) in &mut self.stream {
            if let Some(vals) = self.state.tab.remove(&k) {
                return Some((k, v, vals));
            }
        }
        None
    }
}
impl<'a, K, Buf, BufV, Stream, StreamV> BatchJoin<'a, K, Buf, BufV, Stream, StreamV>
where
    K: Eq + std::hash::Hash,
    Buf: Iterator<Item = (K, BufV)>,
    Stream: Iterator<Item = (K, StreamV)>,
{
    pub fn new(buf: Buf, stream: Stream, state: &'a mut BatchJoinState<K, BufV>) -> Self {
        Self { buf, stream, state }
    }
}

#[derive(Debug)]
pub struct JoinState<K, V1, V2> {
    ltab: HashMap<K, Vec<V1>>,
    rtab: HashMap<K, Vec<V2>>,
    lbuffer: Option<(K, V1, Vec<V2>)>,
    rbuffer: Option<(K, V2, Vec<V1>)>,
}

impl<K, V1, V2> Default for JoinState<K, V1, V2> {
    fn default() -> Self {
        Self {
            ltab: HashMap::new(),
            rtab: HashMap::new(),
            lbuffer: None,
            rbuffer: None,
        }
    }
}

pub struct SymmetricHashJoin<'a, K, I1, V1, I2, V2>
where
    K: Eq + std::hash::Hash + Clone,
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = (K, V1)>,
    I2: Iterator<Item = (K, V2)>,
{
    lhs: I1,
    rhs: I2,
    state: &'a mut JoinState<K, V1, V2>,
}

impl<'a, K, I1, V1, I2, V2> Iterator for SymmetricHashJoin<'a, K, I1, V1, I2, V2>
where
    K: Eq + std::hash::Hash + Clone,
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = (K, V1)>,
    I2: Iterator<Item = (K, V2)>,
{
    type Item = (K, V1, V2);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((k, v, vs)) = &mut self.state.lbuffer {
                // TODO(justin): unnecessary clone (sometimes).
                let result = (k.clone(), v.clone(), vs.pop().unwrap());
                if vs.is_empty() {
                    self.state.lbuffer = None;
                }
                return Some(result);
            } else if let Some((k, v, vs)) = &mut self.state.rbuffer {
                // TODO(justin): unnecessary clone (sometimes).
                let result = (k.clone(), vs.pop().unwrap(), v.clone());
                if vs.is_empty() {
                    self.state.rbuffer = None;
                }
                return Some(result);
            }

            if let Some((k, v1)) = self.lhs.next() {
                let vec = self.state.ltab.entry(k.clone()).or_insert_with(Vec::new);
                if !vec.contains(&v1) {
                    vec.push(v1.clone());
                    if let Some(vs) = self.state.rtab.get(&k) {
                        self.state.lbuffer = Some((k, v1, vs.clone()));
                    }
                }
                continue;
            }

            if let Some((k, v2)) = self.rhs.next() {
                let vec = self.state.rtab.entry(k.clone()).or_insert_with(Vec::new);
                if !vec.contains(&v2) {
                    vec.push(v2.clone());
                    if let Some(vs) = self.state.ltab.get(&k) {
                        self.state.rbuffer = Some((k, v2, vs.clone()));
                    }
                }
                continue;
            }
            return None;
        }
    }
}
impl<'a, K, I1, V1, I2, V2> SymmetricHashJoin<'a, K, I1, V1, I2, V2>
where
    K: Eq + std::hash::Hash + Clone,
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = (K, V1)>,
    I2: Iterator<Item = (K, V2)>,
{
    pub fn new(lhs: I1, rhs: I2, state: &'a mut JoinState<K, V1, V2>) -> Self {
        Self { lhs, rhs, state }
    }
}

pub struct CrossJoinState<V1, V2> {
    ltab: Vec<V1>,
    rtab: Vec<V2>,
    draw_from_left: bool,
    opposite_ix: Range<usize>,
}

impl<'a, V1, V2> Default for CrossJoinState<V1, V2> {
    fn default() -> Self {
        Self {
            ltab: Vec::new(),
            rtab: Vec::new(),
            draw_from_left: true,
            opposite_ix: 0..0,
        }
    }
}

pub struct CrossJoin<'a, I1, V1, I2, V2>
where
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = V1>,
    I2: Iterator<Item = V2>,
{
    lhs: I1,
    rhs: I2,
    state: &'a mut CrossJoinState<V1, V2>,
}

impl<'a, I1, V1: 'static, I2, V2: 'static> Iterator for CrossJoin<'a, I1, V1, I2, V2>
where
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = V1>,
    I2: Iterator<Item = V2>,
{
    type Item = (V1, V2);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // see if there's a match from the opposite's iterator
            if let Some(i) = self.state.opposite_ix.next() {
                if self.state.draw_from_left {
                    let l = self.state.ltab.last().unwrap().clone();
                    let r = self.state.rtab.get(i).unwrap().clone();
                    return Some((l, r));
                } else {
                    let l = self.state.ltab.get(i).unwrap().clone();
                    let r = self.state.rtab.last().unwrap().clone();
                    return Some((l, r));
                }
            }
            // else fetch a new tuple, alternating the sides we fetch from,
            // so we draw from each input at the same rate.
            let mut found_new = false;
            for _i in ["opposite", "same"] {
                // toggle sides
                self.state.draw_from_left = !self.state.draw_from_left;

                // try to fetch from the specified side
                #[allow(clippy::collapsible_else_if)]
                if self.state.draw_from_left {
                    if let Some(l) = self.lhs.next() {
                        self.state.draw_from_left = true;
                        self.state.ltab.push(l);
                        self.state.opposite_ix = 0..self.state.rtab.len();
                        found_new = true;
                        break;
                    }
                } else {
                    if let Some(r) = self.rhs.next() {
                        self.state.draw_from_left = false;
                        self.state.rtab.push(r);
                        self.state.opposite_ix = 0..self.state.ltab.len();
                        found_new = true;
                        break;
                    }
                }
            }
            if !found_new {
                return None;
            }
        }
    }
}
impl<'a, I1, V1, I2, V2> CrossJoin<'a, I1, V1, I2, V2>
where
    V1: Eq + Clone,
    V2: Eq + Clone,
    I1: Iterator<Item = V1>,
    I2: Iterator<Item = V2>,
{
    pub fn new(lhs: I1, rhs: I2, state: &'a mut CrossJoinState<V1, V2>) -> Self {
        Self { lhs, rhs, state }
    }
}

#[cfg(test)]
mod tests {
    use crate::compiled::pull::{CrossJoin, CrossJoinState, JoinState, SymmetricHashJoin};

    #[test]
    fn hash_join() {
        let lhs = (0..10).map(|x| (x, format!("left {}", x)));
        let rhs = (6..15).map(|x| (x / 2, format!("right {} / 2", x)));

        let mut state = JoinState::default();
        let join = SymmetricHashJoin::new(lhs, rhs, &mut state);

        assert_eq!(
            join.collect::<Vec<_>>(),
            vec![
                (3, "left 3".into(), "right 6 / 2".into()),
                (3, "left 3".into(), "right 7 / 2".into()),
                (4, "left 4".into(), "right 8 / 2".into()),
                (4, "left 4".into(), "right 9 / 2".into()),
                (5, "left 5".into(), "right 10 / 2".into()),
                (5, "left 5".into(), "right 11 / 2".into()),
                (6, "left 6".into(), "right 12 / 2".into()),
                (6, "left 6".into(), "right 13 / 2".into()),
                (7, "left 7".into(), "right 14 / 2".into())
            ]
        );
    }

    #[test]
    fn cross_join() {
        let lhs = (0..3).map(|x| (format!("left {}", x)));
        let rhs = (10..13).map(|x| (format!("right {}", x)));

        let mut state = CrossJoinState::default();
        let join = CrossJoin::new(lhs, rhs, &mut state);

        assert_eq!(
            join.collect::<Vec<_>>(),
            vec![
                ("left 0".into(), "right 10".into()),
                ("left 0".into(), "right 11".into()),
                ("left 1".into(), "right 10".into()),
                ("left 1".into(), "right 11".into()),
                ("left 0".into(), "right 12".into()),
                ("left 1".into(), "right 12".into()),
                ("left 2".into(), "right 10".into()),
                ("left 2".into(), "right 11".into()),
                ("left 2".into(), "right 12".into())
            ]
        );
    }
}

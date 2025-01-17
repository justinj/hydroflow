use std::marker::PhantomData;

pub trait Pusherator: Sized {
    type Item;
    fn give(&mut self, item: Self::Item);

    fn map<F, T>(self, f: F) -> Map<T, Self::Item, F, Self>
    where
        F: Fn(T) -> Self::Item,
    {
        Map::new(f, self)
    }
}

pub struct Pivot<T, I, P>
where
    I: Iterator<Item = T>,
    P: Pusherator<Item = T>,
{
    pull: I,
    push: P,
}
impl<T, I, P> Pivot<T, I, P>
where
    I: Iterator<Item = T>,
    P: Pusherator<Item = T>,
{
    pub fn new(pull: I, push: P) -> Self {
        Self { pull, push }
    }

    pub fn step(&mut self) -> bool {
        if let Some(v) = self.pull.next() {
            self.push.give(v);
            true
        } else {
            false
        }
    }

    pub fn run(mut self) {
        while let Some(v) = self.pull.next() {
            self.push.give(v);
        }
    }
}

pub struct ForEach<T, F>
where
    F: FnMut(T),
{
    f: F,
    _marker: PhantomData<T>,
}
impl<T, F> Pusherator for ForEach<T, F>
where
    F: FnMut(T),
{
    type Item = T;
    fn give(&mut self, item: Self::Item) {
        (self.f)(item)
    }
}
impl<T, F> ForEach<T, F>
where
    F: FnMut(T),
{
    pub fn new(f: F) -> Self {
        ForEach {
            f,
            _marker: PhantomData,
        }
    }
}

pub struct Map<T, U, F, O>
where
    F: Fn(T) -> U,
    O: Pusherator<Item = U>,
{
    out: O,
    f: F,
    _marker: PhantomData<T>,
}
impl<T, U, F, O> Pusherator for Map<T, U, F, O>
where
    F: Fn(T) -> U,
    O: Pusherator<Item = U>,
{
    type Item = T;
    fn give(&mut self, item: Self::Item) {
        self.out.give((self.f)(item));
    }
}
impl<T, U, F, O> Map<T, U, F, O>
where
    F: Fn(T) -> U,
    O: Pusherator<Item = U>,
{
    pub fn new(f: F, out: O) -> Self {
        Map {
            out,
            f,
            _marker: PhantomData,
        }
    }
}

pub struct Filter<T, F, O>
where
    F: Fn(&T) -> bool,
    O: Pusherator<Item = T>,
{
    out: O,
    f: F,
    _marker: PhantomData<T>,
}
impl<T, F, O> Pusherator for Filter<T, F, O>
where
    F: Fn(&T) -> bool,
    O: Pusherator<Item = T>,
{
    type Item = T;
    fn give(&mut self, item: Self::Item) {
        if (self.f)(&item) {
            self.out.give(item);
        }
    }
}
impl<T, F, O> Filter<T, F, O>
where
    F: Fn(&T) -> bool,
    O: Pusherator<Item = T>,
{
    pub fn new(f: F, out: O) -> Self {
        Filter {
            out,
            f,
            _marker: PhantomData,
        }
    }
}

pub struct Partition<T, F, O1, O2>
where
    F: Fn(&T) -> bool,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    out1: O1,
    out2: O2,
    f: F,
    _marker: PhantomData<T>,
}
impl<T, F, O1, O2> Pusherator for Partition<T, F, O1, O2>
where
    F: Fn(&T) -> bool,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    type Item = T;
    fn give(&mut self, item: Self::Item) {
        if (self.f)(&item) {
            self.out1.give(item);
        } else {
            self.out2.give(item);
        }
    }
}
impl<T, F, O1, O2> Partition<T, F, O1, O2>
where
    F: Fn(&T) -> bool,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    pub fn new(f: F, out1: O1, out2: O2) -> Self {
        Partition {
            out1,
            out2,
            f,
            _marker: PhantomData,
        }
    }
}

pub struct Tee<T, O1, O2>
where
    T: Clone,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    out1: O1,
    out2: O2,
    _marker: PhantomData<T>,
}
impl<T, O1, O2> Pusherator for Tee<T, O1, O2>
where
    T: Clone,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    type Item = T;
    fn give(&mut self, item: Self::Item) {
        self.out1.give(item.clone());
        self.out2.give(item);
    }
}
impl<T, O1, O2> Tee<T, O1, O2>
where
    T: Clone,
    O1: Pusherator<Item = T>,
    O2: Pusherator<Item = T>,
{
    pub fn new(out1: O1, out2: O2) -> Self {
        Tee {
            out1,
            out2,
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::compiled::{Filter, ForEach, Map, Partition, Pivot, Pusherator, Tee};

    #[test]
    fn linear_chains() {
        let mut v = Vec::new();
        let mut pusher = Map::new(
            |x| x * 2,
            Filter::new(|x| *x > 5, ForEach::new(|x| v.push(x))),
        );

        for i in 0..5 {
            pusher.give(i);
        }

        assert_eq!(v, vec![6, 8]);
    }

    #[test]
    fn partition() {
        let mut evens = Vec::new();
        let mut odds = Vec::new();
        let mut pusher = Partition::new(
            |x| x % 2 == 0,
            ForEach::new(|x| evens.push(x)),
            ForEach::new(|x| odds.push(x)),
        );

        for i in 0..5 {
            pusher.give(i);
        }

        assert_eq!(evens, vec![0, 2, 4]);
        assert_eq!(odds, vec![1, 3]);
    }

    #[test]
    fn tee() {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut pusher = Tee::new(
            ForEach::new(|x| left.push(x)),
            ForEach::new(|x| right.push(x)),
        );

        for i in 0..5 {
            pusher.give(i);
        }

        assert_eq!(left, vec![0, 1, 2, 3, 4]);
        assert_eq!(right, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn tee_rcs() {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut pusher = Map::new(
            |x| Rc::new(x),
            Tee::new(
                ForEach::new(|x: Rc<i32>| left.push(*x)),
                ForEach::new(|x: Rc<i32>| right.push(*x)),
            ),
        );

        for i in 0..5 {
            pusher.give(i);
        }

        assert_eq!(left, vec![0, 1, 2, 3, 4]);
        assert_eq!(right, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn pivot() {
        let a = 0..10;
        let b = 10..20;

        let mut left = Vec::new();
        let mut right = Vec::new();

        let pivot = Pivot::new(
            a.into_iter().chain(b.into_iter()),
            Partition::new(
                |x| x % 2 == 0,
                ForEach::new(|x| left.push(x)),
                ForEach::new(|x| right.push(x)),
            ),
        );

        pivot.run();

        assert_eq!(left, vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);
        assert_eq!(right, vec![1, 3, 5, 7, 9, 11, 13, 15, 17, 19]);
    }
}

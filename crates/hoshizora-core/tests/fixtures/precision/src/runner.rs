pub struct Runner;

impl Runner {
    pub fn dispatch(&self, kind: u8, n: usize) -> usize {
        match kind {
            0 => self.quad(n),
            1 => self.quad(n) + 1,
            2 => self.quad(n) + 2,
            3 => self.quad(n) + 3,
            4 => self.quad(n) + 4,
            5 => self.quad(n) + 5,
            6 => self.quad(n) + 6,
            7 => self.quad(n) + 7,
            8 => self.quad(n) + 8,
            9 => self.quad(n) + 9,
            10 => self.quad(n) + 10,
            11 => self.quad(n) + 11,
            12 => self.quad(n) + 12,
            13 => self.quad(n) + 13,
            14 => self.quad(n) + 14,
            15 => self.quad(n) + 15,
            _ => 0,
        }
    }

    pub fn quad(&self, n: usize) -> usize {
        let mut total = 0;
        for i in 0..n {
            for j in 0..n {
                total += i * j;
            }
        }
        total
    }

    pub fn repeat(&self, n: usize) -> usize {
        let mut total = 0;
        for _ in 0..n {
            total += self.quad(n);
        }
        total
    }

    pub fn repeat_unknown(&self, n: usize) -> usize {
        let other = make_other();
        let mut total = 0;
        for _ in 0..n {
            total += other.heavy(n);
        }
        total
    }
}

fn make_other() -> Other {
    Other
}

pub struct Other;

impl Other {
    pub fn heavy(&self, n: usize) -> usize {
        let mut total = 0;
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    total += i * j * k;
                }
            }
        }
        total
    }
}

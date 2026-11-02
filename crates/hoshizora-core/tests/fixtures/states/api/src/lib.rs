pub enum Status {
    Active,
    Paused,
    Done,
}

pub fn label(s: &Status) -> &'static str {
    match s {
        Status::Active => "a",
        Status::Paused => "p",
        _ => "",
    }
}

pub fn closed(s: &Status) -> bool {
    match s {
        Status::Active | Status::Paused => false,
        Status::Done => true,
    }
}

pub fn only_active(s: &Status) -> Option<u8> {
    match s {
        Status::Active => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Status;

    #[test]
    fn label_in_test() {
        let s = Status::Done;
        let _ = match s {
            Status::Active => 1,
            Status::Paused => 2,
            _ => 0,
        };
    }
}

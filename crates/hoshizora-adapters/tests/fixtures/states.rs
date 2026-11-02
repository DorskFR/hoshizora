pub enum Status {
    Active,
    Paused(u32),
    Done { at: u64 },
}

pub enum Single {
    Only,
}

fn label(s: &Status) -> &'static str {
    match s {
        Status::Active | Status::Paused(_) => "a",
        Status::Done { .. } if true => "d",
        _ => "",
    }
}

fn is_live(s: Status) -> bool {
    if s == Status::Active {
        true
    } else if matches!(s, Status::Paused(_) | Status::Done { .. }) {
        false
    } else {
        unreachable!()
    }
}

fn pairs(a: u8, b: u8) -> u8 {
    match (a, b) {
        (1, _) => 1,
        _ => 0,
    }
}

fn code(c: &str) -> u8 {
    match c {
        "x" => 1,
        "y" => 2,
        other => return 0,
    }
}

fn opt(o: Option<Status>) -> u8 {
    match o {
        Some(Status::Active) => 1,
        None => 2,
        _ => 3,
    }
}

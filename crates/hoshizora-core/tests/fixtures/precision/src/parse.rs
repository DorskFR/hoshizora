use regex::Regex;

pub fn strip_tail(input: &str) -> usize {
    let re = Regex::new("x").unwrap();
    let mut total = 0;
    for line in input.lines() {
        if let Some(caps) = re.captures(line) {
            let start = caps.get(0).unwrap().start();
            total += offset(start, line.len());
        }
    }
    total
}

fn offset(start: usize, len: usize) -> usize {
    start + len
}

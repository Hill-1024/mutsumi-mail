pub fn normalize_subject(subject: &str) -> String {
    let mut value = subject.trim().to_string();
    loop {
        let lower = value.to_ascii_lowercase();
        let prefixes = ["re:", "fw:", "fwd:"];
        if let Some(prefix) = prefixes.iter().find(|prefix| lower.starts_with(*prefix)) {
            value = value[prefix.len()..].trim_start().to_string();
        } else {
            break;
        }
    }
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::normalize_subject;

    #[test]
    fn strips_nested_reply_prefixes() {
        assert_eq!(normalize_subject("Re: Fwd:  Q4 review"), "q4 review");
    }
}

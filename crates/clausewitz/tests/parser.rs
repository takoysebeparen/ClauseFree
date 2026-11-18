use clausewitz::{DiagKind, Item, LineIndex, Operator, Value, decode, parse};

fn entry_keys<'a>(block: &clausewitz::Block<'a>) -> Vec<&'a str> {
    block.entries().map(|e| e.key.text).collect()
}

#[test]
fn entries_keep_order_and_duplicates() {
    let p = parse("b = 1\na = 2\nb = 3\n");
    assert!(p.is_clean());
    assert_eq!(entry_keys(&p.root), vec!["b", "a", "b"]);
    let all: Vec<_> = p.root.get_all("b").filter_map(|e| e.value.as_str()).collect();
    assert_eq!(all, vec!["1", "3"]);
    assert_eq!(p.root.get("a").and_then(|e| e.value.as_scalar()).and_then(|s| s.as_i64()), Some(2));
    assert!(p.root.get("missing").is_none());
}

#[test]
fn all_operators_are_recognised() {
    let p = parse("a = 1 b == 2 c != 3 d < 4 e > 5 f <= 6 g >= 7 h ?= 8 i=9 j<10 k>=11 l?=12 m!=13");
    assert!(p.is_clean(), "{:?}", p.diagnostics);
    let ops: Vec<Operator> = p.root.entries().map(|e| e.op).collect();
    use Operator::*;
    assert_eq!(ops, vec![Eq, EqEq, NotEq, Lt, Gt, Le, Ge, OptionalEq, Eq, Lt, Ge, OptionalEq, NotEq]);
    let values: Vec<_> = p.root.entries().filter_map(|e| e.value.as_str()).collect();
    assert_eq!(values.len(), 13);
    assert_eq!(values[12], "13");
}

#[test]
fn comments_bom_and_crlf() {
    let p = parse("\u{feff}# header comment\r\na = 1 # trailing\r\n  # indented\r\nb = 2\r\n");
    assert!(p.is_clean());
    assert_eq!(entry_keys(&p.root), vec!["a", "b"]);
    // decode() strips the BOM as well.
    let text = decode(b"\xEF\xBB\xBFx = 1");
    assert_eq!(&*text, "x = 1");
}

#[test]
fn quoted_strings_keep_special_characters() {
    let p = parse(r#"name = "A # not a comment { } = <" desc = "say \"hi\"" empty = "" "quoted key" = x"#);
    assert!(p.is_clean(), "{:?}", p.diagnostics);
    let name = p.root.get("name").and_then(|e| e.value.as_scalar()).expect("name");
    assert!(name.quoted);
    assert_eq!(name.text, "A # not a comment { } = <");
    let desc = p.root.get("desc").and_then(|e| e.value.as_scalar()).expect("desc");
    assert_eq!(desc.text, r#"say \"hi\""#);
    assert_eq!(desc.unescaped(), "say \"hi\"");
    assert_eq!(p.root.get("empty").and_then(|e| e.value.as_str()), Some(""));
    assert!(p.root.get("quoted key").is_some());
}

#[test]
fn nested_blocks_lists_and_tagged_values() {
    let src = "country = {\n tag = GER\n color = rgb { 255 0 0 }\n list = { 1 2 3 }\n mixed = { a = 1 b c }\n blocks = { { x = 1 } { x = 2 } }\n empty = { }\n}";
    let p = parse(src);
    assert!(p.is_clean(), "{:?}", p.diagnostics);
    let country = p.root.get("country").and_then(|e| e.value.as_block()).expect("country");
    let color = &country.get("color").expect("color").value;
    match color {
        Value::Tagged(tag, block) => {
            assert_eq!(tag.text, "rgb");
            assert_eq!(block.values().filter_map(|v| v.as_str()).collect::<Vec<_>>(), vec!["255", "0", "0"]);
        }
        other => panic!("expected a tagged value, got {other:?}"),
    }
    let list = country.get("list").and_then(|e| e.value.as_block()).expect("list");
    assert_eq!(list.values().count(), 3);
    let mixed = country.get("mixed").and_then(|e| e.value.as_block()).expect("mixed");
    assert_eq!((mixed.entries().count(), mixed.values().count()), (1, 2));
    let blocks = country.get("blocks").and_then(|e| e.value.as_block()).expect("blocks");
    assert_eq!(blocks.values().filter(|v| v.as_block().is_some()).count(), 2);
    assert!(country.get("empty").and_then(|e| e.value.as_block()).is_some_and(|b| b.is_empty()));
}

#[test]
fn inline_math_and_script_variables() {
    let p = parse("@gap = 5\nsize = @[ gap * 2 + 1 ]\nnext = @gap");
    assert!(p.is_clean(), "{:?}", p.diagnostics);
    assert_eq!(p.root.get("@gap").and_then(|e| e.value.as_str()), Some("5"));
    assert_eq!(p.root.get("size").and_then(|e| e.value.as_str()), Some("@[ gap * 2 + 1 ]"));
    assert_eq!(p.root.get("next").and_then(|e| e.value.as_str()), Some("@gap"));
}

#[test]
fn scalar_helpers() {
    let p = parse("a = yes b = no c = -12 d = 1.5 e = \"yes\"");
    let get = |k: &str| p.root.get(k).and_then(|e| e.value.as_scalar()).copied().expect("scalar");
    assert_eq!(get("a").as_bool(), Some(true));
    assert_eq!(get("b").as_bool(), Some(false));
    assert_eq!(get("c").as_i64(), Some(-12));
    assert_eq!(get("d").as_i64(), None, "decimals are not integers");
    assert_eq!(get("e").as_bool(), None, "quoted text is not a boolean");
}

#[test]
fn spans_point_at_the_source() {
    let src = "a = 1\nb = { c = 2 }\n";
    let p = parse(src);
    let b = p.root.get("b").expect("b");
    let text = &src[b.key.span.start as usize..b.key.span.end as usize];
    assert_eq!(text, "b");
    let block = b.value.as_block().expect("block");
    assert_eq!(&src[block.span.start as usize..block.span.end as usize], "{ c = 2 }");
    let index = LineIndex::new(src);
    assert_eq!(index.locate(b.key.span.start), (2, 1));
    assert_eq!(index.locate(0), (1, 1));
    assert_eq!(index.locate(8), (2, 3));
}

#[test]
fn unclosed_brace_is_reported_and_the_tree_is_kept() {
    let p = parse("a = { b = 1 c = { d = 2 ");
    assert!(p.diagnostics.iter().any(|d| d.kind == DiagKind::UnclosedBrace));
    let a = p.root.get("a").and_then(|e| e.value.as_block()).expect("a");
    assert_eq!(a.get("b").and_then(|e| e.value.as_str()), Some("1"));
}

#[test]
fn stray_close_brace_does_not_swallow_the_rest() {
    let p = parse("a = 1 } b = 2");
    assert_eq!(p.diagnostics.iter().filter(|d| d.kind == DiagKind::UnexpectedCloseBrace).count(), 1);
    assert_eq!(entry_keys(&p.root), vec!["a", "b"]);
}

#[test]
fn missing_value_and_stray_operator() {
    let p = parse("a = } b = 1");
    assert!(p.diagnostics.iter().any(|d| d.kind == DiagKind::MissingValue));
    let q = parse("= 5 c = 1");
    assert!(q.diagnostics.iter().any(|d| d.kind == DiagKind::UnexpectedOperator));
    assert!(q.root.get("c").is_some());
    let r = parse("a =");
    assert!(r.diagnostics.iter().any(|d| d.kind == DiagKind::MissingValue));
    assert!(matches!(r.root.items[0], Item::Entry(ref e) if matches!(e.value, Value::Missing(_))));
}

#[test]
fn unterminated_string_and_math() {
    let p = parse("a = \"never closed\nb = 1");
    assert!(p.diagnostics.iter().any(|d| d.kind == DiagKind::UnterminatedString));
    let q = parse("a = @[ 1 + ");
    assert!(q.diagnostics.iter().any(|d| d.kind == DiagKind::UnterminatedMath));
}

#[test]
fn absurd_nesting_does_not_overflow_the_stack() {
    let deep = "{".repeat(100_000);
    let p = parse(&deep);
    assert!(p.diagnostics.iter().any(|d| d.kind == DiagKind::TooDeep));
    let closed = format!("{}{}", "a = {".repeat(5_000), "}".repeat(5_000));
    let q = parse(&closed);
    assert!(q.diagnostics.iter().any(|d| d.kind == DiagKind::TooDeep));
    // A normal file after a too-deep block is still parsed.
    let mixed = format!("{} }} tail = 1", "x = {".repeat(300));
    assert!(!parse(&mixed).root.items.is_empty());
}

#[test]
fn garbage_never_panics() {
    // Deterministic pseudo-random strings over the characters that matter to the grammar.
    let alphabet: Vec<&str> = vec![
        "{", "}", "=", "==", "!=", "<", ">", "<=", ">=", "?=", "\"", "\\", "#", "@[", "]", "@", " ", "\n", "a", "1",
        "é", "日", "\u{feff}",
    ];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..3_000 {
        let len = (next() % 60) as usize;
        let text: String = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        let parsed = parse(&text);
        // Every span must stay inside the text, so error messages can always be rendered.
        for d in &parsed.diagnostics {
            assert!(d.span.start <= d.span.end && d.span.end as usize <= text.len(), "{text:?} -> {d:?}");
        }
    }
    // Invalid UTF-8 goes through decode().
    let bytes = [0xff, 0xfe, b'a', b'=', 0xc0, b'\n'];
    let _ = parse(&decode(&bytes));
}

/// Parses a few real files when `HOI4_DIR` points at an installation. Skipped otherwise.
#[test]
fn real_files_parse_cleanly_when_the_game_is_available() {
    let Ok(dir) = std::env::var("HOI4_DIR") else {
        return;
    };
    let root = std::path::Path::new(&dir);
    for rel in
        ["common/units/infantry.txt", "common/ideologies/00_ideologies.txt", "history/countries/GER - Germany.txt"]
    {
        let Ok(bytes) = std::fs::read(root.join(rel)) else {
            continue;
        };
        let text = decode(&bytes);
        let parsed = parse(&text);
        assert!(parsed.is_clean(), "{rel}: {:?}", parsed.diagnostics.first());
        assert!(!parsed.root.is_empty(), "{rel} parsed as empty");
    }
}

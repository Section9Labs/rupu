use rupu_skills::{NameError, SkillName};

#[test]
fn valid_names_parse() {
    for n in [
        "pdf-processing",
        "a",
        "code-review",
        "x1-2y",
        &"a".repeat(64),
    ] {
        let parsed: SkillName = n.parse().unwrap();
        assert_eq!(parsed.as_str(), n);
        assert_eq!(parsed.to_string(), n);
    }
}

#[test]
fn invalid_names_say_why() {
    assert_eq!("".parse::<SkillName>(), Err(NameError::Empty));
    assert_eq!(
        "a".repeat(65).parse::<SkillName>(),
        Err(NameError::TooLong(65))
    );
    assert_eq!(
        "PDF-Processing".parse::<SkillName>(),
        Err(NameError::BadChar('P'))
    );
    assert_eq!("pdf_x".parse::<SkillName>(), Err(NameError::BadChar('_')));
    assert_eq!("pdf.x".parse::<SkillName>(), Err(NameError::BadChar('.')));
    assert_eq!("-pdf".parse::<SkillName>(), Err(NameError::EdgeHyphen));
    assert_eq!("pdf-".parse::<SkillName>(), Err(NameError::EdgeHyphen));
    assert_eq!(
        "pdf--processing".parse::<SkillName>(),
        Err(NameError::DoubleHyphen)
    );
}

#[test]
fn serializes_as_a_plain_string() {
    let n: SkillName = "code-review".parse().unwrap();
    assert_eq!(serde_json::to_string(&n).unwrap(), "\"code-review\"");
}

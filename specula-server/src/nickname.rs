/// What a visitor is called in a match (D10). The rule is the vanilla
/// lobby's for a new account (`gui/prelobby/common/credentials/
/// credentials.js`), so a name reads the same as in the vanilla lobby and
/// cannot impersonate another by look-alike characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nickname(String);

pub const MIN_LENGTH: usize = 3;
pub const MAX_LENGTH: usize = 20;

impl Nickname {
    /// `/^(?=.*[a-z])[a-z0-9._-]{3,20}$/i`.
    pub fn parse(text: &str) -> Option<Nickname> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        let valid = (MIN_LENGTH..=MAX_LENGTH).contains(&text.len())
            && text.chars().all(allowed)
            && text.chars().any(|c| c.is_ascii_alphabetic());
        valid.then(|| Nickname(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Gives every name that repeats an earlier one a `_N` suffix, N from 2,
/// skipping any that would itself repeat a name (D10). Compared ignoring
/// case, as the lobby's XMPP names are. A suffixed name may pass
/// `MAX_LENGTH`: the limit is on what a visitor types, and cutting the
/// name instead could make it another visitor's.
pub fn suffix_duplicates(names: &mut [Nickname]) {
    let taken = |names: &[Nickname], candidate: &str| {
        names
            .iter()
            .any(|name| name.0.eq_ignore_ascii_case(candidate))
    };
    for index in 1..names.len() {
        let (earlier, rest) = names.split_at_mut(index);
        let name = &mut rest[0];
        if !taken(earlier, &name.0) {
            continue;
        }
        let base = std::mem::take(&mut name.0);
        let mut count = 2;
        loop {
            let candidate = format!("{base}_{count}");
            // Later names too: "bob", "bob", "bob_2" gives the second
            // "bob" "bob_3", so the third keeps its own name.
            let later = &rest[1..];
            if !taken(earlier, &candidate) && !taken(later, &candidate) {
                rest[0].0 = candidate;
                break;
            }
            count += 1;
        }
    }
}

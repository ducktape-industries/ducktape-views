use duck_address::{Address, ChainId, number};

const MAX_REPO_NAME_LEN: usize = 64;

pub(super) struct AccountAddress {
    pub(super) account: u64,
}

impl AccountAddress {
    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        Address::new(chain, "identity", vec![self.account.to_string()]).ok()
    }
}

pub(super) struct ForgeRepoAddress {
    pub(super) owner: String,
    pub(super) repo: String,
}

impl ForgeRepoAddress {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name.split_once('/') {
            Some((owner, repo)) if !repo.contains('/') => Self::named(owner, repo),
            _ => None,
        }
    }

    pub(super) fn from_address(address: &Address) -> Option<Self> {
        if address.module != "forge" {
            return None;
        }
        let [owner, repo] = address.path.as_slice() else {
            return None;
        };
        Self::named(owner, repo)
    }

    pub(super) fn name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    fn named(owner: &str, repo: &str) -> Option<Self> {
        if repo.ends_with(".git") {
            return None;
        }
        Some(Self {
            owner: name(owner)?,
            repo: name(repo)?,
        })
    }
}

pub(super) struct ForgeLocator {
    pub(super) repo: ForgeRepoAddress,
    pub(super) target: ForgeTarget,
}

impl ForgeLocator {
    pub(super) fn from_address(address: &Address) -> Option<Self> {
        if address.module != "forge" {
            return None;
        }
        let [owner, repo, rest @ ..] = address.path.as_slice() else {
            return None;
        };
        let repo = ForgeRepoAddress::named(owner, repo)?;
        let target = match rest {
            [item] => ForgeTarget::Item {
                number: number(item)?,
            },
            [item, keyword, seq] if keyword == "comment" => ForgeTarget::Comment {
                number: number(item)?,
                seq: number(seq)?,
            },
            [keyword, rev, path @ ..] if keyword == "blob" && !path.is_empty() => {
                ForgeTarget::Blob {
                    rev: commit(rev)?,
                    path: path.to_vec(),
                }
            }
            _ => return None,
        };
        Some(Self { repo, target })
    }

    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        let mut path = vec![self.repo.owner.clone(), self.repo.repo.clone()];
        match &self.target {
            ForgeTarget::Item { number } => path.push(number.to_string()),
            ForgeTarget::Comment { number, seq } => {
                path.extend([number.to_string(), "comment".into(), seq.to_string()])
            }
            ForgeTarget::Blob { rev, path: file } => {
                path.extend(["blob".into(), rev.clone()]);
                path.extend(file.iter().cloned());
            }
        }
        let address = Address::new(chain, "forge", path).ok()?;
        Self::from_address(&address)?;
        Some(address)
    }
}

pub(super) enum ForgeTarget {
    Item { number: u64 },
    Comment { number: u64, seq: u64 },
    Blob { rev: String, path: Vec<String> },
}

fn commit(rev: &str) -> Option<String> {
    (rev.len() == 40
        && rev
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
    .then(|| rev.to_owned())
}

fn name(value: &str) -> Option<String> {
    (matches!(value.len(), 1..=MAX_REPO_NAME_LEN)
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-')))
    .then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ChainId {
        "testnet#0a1b2c3d".parse().expect("a chain id")
    }

    fn address(text: &str) -> Address {
        Address::parse(text).expect("generic address")
    }

    #[test]
    fn construction_keeps_repo_item_and_account_spellings() {
        let repo = ForgeRepoAddress::from_name("ducks/core").expect("repo");
        assert_eq!(
            ForgeLocator {
                repo,
                target: ForgeTarget::Item { number: 7 },
            }
            .address(chain())
            .expect("item")
            .to_string(),
            "duck://testnet-0a1b2c3d/forge/ducks/core/7"
        );
        assert_eq!(
            AccountAddress { account: 7 }
                .address(chain())
                .expect("account")
                .to_string(),
            "duck://testnet-0a1b2c3d/identity/7"
        );
    }

    #[test]
    fn parsing_keeps_all_locator_variants() {
        let commit = "1111222233334444555566667777888899990000";
        let cases = [
            "duck://testnet-0a1b2c3d/forge/ducks/core/7",
            "duck://testnet-0a1b2c3d/forge/ducks/core/7/comment/3",
            "duck://testnet-0a1b2c3d/forge/ducks/core/blob/1111222233334444555566667777888899990000/src/main.rs",
        ];
        for text in cases {
            assert!(
                ForgeLocator::from_address(&address(text)).is_some(),
                "{text}"
            );
        }
        assert!(
            ForgeLocator::from_address(&address(&format!(
                "duck://testnet-0a1b2c3d/forge/ducks/core/blob/{commit}/src/main.rs"
            )))
            .is_some()
        );
    }

    #[test]
    fn parsing_preserves_module_name_and_shape_restrictions() {
        for text in [
            "duck://testnet-0a1b2c3d/pages/ducks/core/7",
            "duck://testnet-0a1b2c3d/forge/ducks/core/007",
            "duck://testnet-0a1b2c3d/forge/ducks/core/7/comment/003",
            "duck://testnet-0a1b2c3d/forge/ducks/core/blob/main/src/main.rs",
            "duck://testnet-0a1b2c3d/forge/ducks/.git/7",
        ] {
            let address = address(text);
            assert!(ForgeRepoAddress::from_address(&address).is_none(), "{text}");
            assert!(ForgeLocator::from_address(&address).is_none(), "{text}");
        }
        let bare = address("duck://testnet-0a1b2c3d/forge/ducks/core");
        assert!(ForgeRepoAddress::from_address(&bare).is_some());
        assert!(ForgeLocator::from_address(&bare).is_none());
        assert!(ForgeRepoAddress::from_name("core").is_none());
        assert!(ForgeRepoAddress::from_name("ducks/core.git").is_none());
    }
}

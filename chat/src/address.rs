use duck_address::{Address, ChainId, number};

pub(super) struct MessageAddress {
    pub(super) channel: String,
    pub(super) seq: Option<u64>,
}

impl MessageAddress {
    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        let mut path = vec![self.channel];
        path.extend(self.seq.map(|seq| seq.to_string()));
        let address = Address::new(chain, "chat", path).ok()?;
        Self::from_address(&address)?;
        Some(address)
    }

    fn from_address(address: &Address) -> Option<Self> {
        if address.module != "chat" {
            return None;
        }
        match address.path.as_slice() {
            [channel] => Some(Self {
                channel: channel.clone(),
                seq: None,
            }),
            [channel, seq] => Some(Self {
                channel: channel.clone(),
                seq: Some(number(seq)?),
            }),
            _ => None,
        }
    }
}

pub(super) struct AccountAddress {
    pub(super) account: u64,
}

impl AccountAddress {
    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        let address = Address::new(chain, "identity", vec![self.account.to_string()]).ok()?;
        Self::from_address(&address)?;
        Some(address)
    }

    fn from_address(address: &Address) -> Option<Self> {
        if address.module != "identity" {
            return None;
        }
        let [account] = address.path.as_slice() else {
            return None;
        };
        number(account).map(|account| Self { account })
    }
}

pub(super) struct RunAddress {
    pub(super) digest: String,
}

impl RunAddress {
    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        let valid = self.digest.len() == 64
            && self
                .digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !valid {
            return None;
        }
        let address = Address::new(chain, "runs", vec![self.digest]).ok()?;
        Self::from_address(&address)?;
        Some(address)
    }

    fn from_address(address: &Address) -> Option<Self> {
        if address.module != "runs" {
            return None;
        }
        let [digest] = address.path.as_slice() else {
            return None;
        };
        (digest.len() == 64
            && digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| Self {
            digest: digest.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ChainId {
        "testnet#0a1b2c3d".parse().expect("a chain id")
    }

    #[test]
    fn chat_and_identity_tails_keep_their_exact_paths() {
        assert_eq!(
            MessageAddress {
                channel: "general".into(),
                seq: Some(42),
            }
            .address(chain())
            .expect("message")
            .to_string(),
            "duck://testnet-0a1b2c3d/chat/general/42"
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
    fn run_tail_keeps_only_dispatch_digests() {
        let digest = "9c46c7d7d32f58a6c81451150055c5109e95fb2a1dd70d23eb564b9b1f28812a";
        assert!(
            RunAddress {
                digest: digest.into()
            }
            .address(chain())
            .is_some()
        );
        for digest in ["", "run-1", &digest.to_uppercase()] {
            assert!(
                RunAddress {
                    digest: digest.into()
                }
                .address(chain())
                .is_none()
            );
        }
    }

    #[test]
    fn tails_reject_the_wrong_module_and_shape() {
        let address = Address::parse("duck://testnet-0a1b2c3d/pages/page").unwrap();
        assert!(MessageAddress::from_address(&address).is_none());
        let address = Address::parse("duck://testnet-0a1b2c3d/chat/general/007").unwrap();
        assert!(MessageAddress::from_address(&address).is_none());
        let address = Address::parse("duck://testnet-0a1b2c3d/identity/007").unwrap();
        assert!(AccountAddress::from_address(&address).is_none());
    }
}

use duck_address::{Address, ChainId};

pub(super) struct PageAddress {
    pub(super) page: String,
    pub(super) block: Option<String>,
}

impl PageAddress {
    pub(super) fn address(self, chain: ChainId) -> Option<Address> {
        let mut path = vec![self.page];
        if let Some(block) = self.block {
            path.extend(["block".to_owned(), block]);
        }
        let address = Address::new(chain, "pages", path).ok()?;
        Self::from_address(&address)?;
        Some(address)
    }

    fn from_address(address: &Address) -> Option<Self> {
        if address.module != "pages" {
            return None;
        }
        match address.path.as_slice() {
            [page] => Some(Self {
                page: page.clone(),
                block: None,
            }),
            [page, keyword, block] if keyword == "block" => Some(Self {
                page: page.clone(),
                block: Some(block.clone()),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ChainId {
        "testnet#0a1b2c3d".parse().expect("a chain id")
    }

    #[test]
    fn page_and_block_identity_keep_their_paths() {
        assert_eq!(
            PageAddress {
                page: "p-1".into(),
                block: None,
            }
            .address(chain())
            .expect("page")
            .to_string(),
            "duck://testnet-0a1b2c3d/pages/p-1"
        );
        assert_eq!(
            PageAddress {
                page: "p-1".into(),
                block: Some("Blk_7".into()),
            }
            .address(chain())
            .expect("block")
            .to_string(),
            "duck://testnet-0a1b2c3d/pages/p-1/block/Blk_7"
        );
    }

    #[test]
    fn page_tail_parsing_keeps_only_the_two_module_forms() {
        for (text, page, block) in [
            ("duck://testnet-0a1b2c3d/pages/p-1", "p-1", None),
            (
                "duck://testnet-0a1b2c3d/pages/p-1/block/Blk_7",
                "p-1",
                Some("Blk_7"),
            ),
        ] {
            let address = Address::parse(text).expect("address");
            let parsed = PageAddress::from_address(&address).expect("page tail");
            assert_eq!(parsed.page, page);
            assert_eq!(parsed.block.as_deref(), block);
        }
        for text in [
            "duck://testnet-0a1b2c3d/chat/p-1",
            "duck://testnet-0a1b2c3d/pages",
            "duck://testnet-0a1b2c3d/pages/p-1/Block/Blk_7",
            "duck://testnet-0a1b2c3d/pages/p-1/block",
        ] {
            let address = Address::parse(text).expect("generic address");
            assert!(PageAddress::from_address(&address).is_none(), "{text}");
        }
    }
}

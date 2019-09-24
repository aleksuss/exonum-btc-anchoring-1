// Copyright 2019 The Exonum Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! BTC anchoring configuration data types.

pub use crate::proto::Config;

use bitcoin::network::constants::Network;
use btc_transaction_utils::{
    multisig::{RedeemScript, RedeemScriptBuilder, RedeemScriptError},
    p2wsh,
};
use exonum::{
    crypto::PublicKey,
    helpers::{Height, ValidateInput},
};

use crate::{
    btc::{self, Address},
    proto::AnchoringKeys,
};

/// Returns sufficient number of keys for the given validators number.
pub fn byzantine_quorum(total: usize) -> usize {
    exonum::node::state::State::byzantine_majority_count(total)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: Network::Testnet,
            anchoring_keys: vec![],
            anchoring_interval: 5_000,
            transaction_fee: 10,
            funding_transaction: None,
        }
    }
}

impl Config {
    /// Create Bitcoin anchoring config instance with default parameters for the
    /// given Bitcoin network and public keys of participants.
    pub fn with_public_keys(
        network: Network,
        keys: impl IntoIterator<Item = AnchoringKeys>,
    ) -> Result<Self, RedeemScriptError> {
        let anchoring_keys = keys.into_iter().collect::<Vec<_>>();
        if anchoring_keys.is_empty() {
            Err(RedeemScriptError::NotEnoughPublicKeys)?;
        }

        Ok(Self {
            network,
            anchoring_keys,
            ..Self::default()
        })
    }

    /// Try to find bitcoin public key corresponding with the given service key.
    pub fn find_bitcoin_key(&self, service_key: &PublicKey) -> Option<(usize, btc::PublicKey)> {
        self.anchoring_keys.iter().enumerate().find_map(|(n, x)| {
            if &x.service_key == service_key {
                Some((n, x.bitcoin_key))
            } else {
                None
            }
        })
    }

    /// Returns the corresponding Bitcoin address.
    pub fn anchoring_address(&self) -> Address {
        p2wsh::address(&self.redeem_script(), self.network).into()
    }

    /// Returns the corresponding redeem script.
    pub fn redeem_script(&self) -> RedeemScript {
        let quorum = byzantine_quorum(self.anchoring_keys.len());
        RedeemScriptBuilder::with_public_keys(self.anchoring_keys.iter().map(|x| x.bitcoin_key.0))
            .quorum(quorum)
            .to_script()
            .unwrap()
    }

    /// Compute the P2WSH output corresponding to the actual redeem script.
    pub fn anchoring_out_script(&self) -> bitcoin::Script {
        self.redeem_script().as_ref().to_v0_p2wsh()
    }

    /// Returns the latest height below the given height which must be anchored.
    pub fn previous_anchoring_height(&self, current_height: Height) -> Height {
        Height(current_height.0 - current_height.0 % self.anchoring_interval)
    }

    /// Returns the nearest height above the given height which must be anchored.
    pub fn following_anchoring_height(&self, current_height: Height) -> Height {
        Height(self.previous_anchoring_height(current_height).0 + self.anchoring_interval)
    }
}

impl ValidateInput for Config {
    type Error = failure::Error;

    fn validate(&self) -> Result<(), Self::Error> {
        // Verify that the redeem script is suitable.
        let quorum = byzantine_quorum(self.anchoring_keys.len());
        let redeem_script = RedeemScriptBuilder::with_public_keys(
            self.anchoring_keys.iter().map(|x| x.bitcoin_key.0),
        )
        .quorum(quorum)
        .to_script()?;
        // TODO Validate other parameters.

        // TODO remove funding transaction from the config.
        if let Some(tx) = self.funding_transaction.as_ref() {
            tx.find_out(&redeem_script.as_ref().to_v0_p2wsh())
                .ok_or_else(|| failure::format_err!("Funding transaction is unsuitable."))?;
        }
        Ok(())
    }
}

mod flatten_keypairs {
    use crate::btc::{PrivateKey, PublicKey};

    use serde_derive::{Deserialize, Serialize};

    use std::collections::HashMap;

    /// The structure for storing the bitcoin keypair.
    /// It is required for reading data from the .toml file into memory.
    #[derive(Deserialize, Serialize)]
    struct BitcoinKeypair {
        /// Bitcoin public key.
        public_key: PublicKey,
        /// Corresponding private key.
        private_key: PrivateKey,
    }

    pub fn serialize<S>(
        keys: &HashMap<PublicKey, PrivateKey>,
        ser: S,
    ) -> ::std::result::Result<S::Ok, S::Error>
    where
        S: ::serde::Serializer,
    {
        use serde::Serialize;

        let keypairs = keys
            .iter()
            .map(|(&public_key, private_key)| BitcoinKeypair {
                public_key,
                private_key: private_key.clone(),
            })
            .collect::<Vec<_>>();
        keypairs.serialize(ser)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HashMap<PublicKey, PrivateKey>, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        use serde::Deserialize;
        Vec::<BitcoinKeypair>::deserialize(deserializer).map(|keypairs| {
            keypairs
                .into_iter()
                .map(|keypair| (keypair.public_key, keypair.private_key))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use exonum::{crypto, helpers::Height};

    use bitcoin::network::constants::Network;
    use btc_transaction_utils::test_data::secp_gen_keypair;

    use crate::proto::AnchoringKeys;

    use super::Config;

    #[test]
    fn test_config_serde() {
        let public_keys = (0..4)
            .map(|_| AnchoringKeys {
                bitcoin_key: secp_gen_keypair(Network::Bitcoin).0.into(),
                service_key: crypto::gen_keypair().0,
            })
            .collect::<Vec<_>>();

        let config = Config::with_public_keys(Network::Bitcoin, public_keys).unwrap();
        assert_eq!(config.redeem_script().content().quorum, 3);

        let json = serde_json::to_value(&config).unwrap();
        let config2: Config = serde_json::from_value(json).unwrap();
        assert_eq!(config2, config);
    }

    #[test]
    fn test_config_anchoring_height() {
        let public_keys = (0..4)
            .map(|_| AnchoringKeys {
                bitcoin_key: secp_gen_keypair(Network::Bitcoin).0.into(),
                service_key: crypto::gen_keypair().0,
            })
            .collect::<Vec<_>>();

        let mut config = Config::with_public_keys(Network::Bitcoin, public_keys).unwrap();
        config.anchoring_interval = 1000;

        assert_eq!(config.previous_anchoring_height(Height(0)), Height(0));
        assert_eq!(config.previous_anchoring_height(Height(999)), Height(0));
        assert_eq!(config.previous_anchoring_height(Height(1000)), Height(1000));
        assert_eq!(config.previous_anchoring_height(Height(1001)), Height(1000));

        assert_eq!(config.following_anchoring_height(Height(0)), Height(1000));
        assert_eq!(config.following_anchoring_height(Height(999)), Height(1000));
        assert_eq!(
            config.following_anchoring_height(Height(1000)),
            Height(2000)
        );
        assert_eq!(
            config.following_anchoring_height(Height(1001)),
            Height(2000)
        );
    }

    // TODO test validation of the Bitcoin anchoring config
}

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

use exonum::{
    blockchain::Schema as CoreSchema,
    crypto::Hash,
    helpers::ValidateInput,
    merkledb::{BinaryValue, Fork},
    runtime::{
        api::ServiceApiBuilder,
        rust::{
            interfaces::{verify_caller_is_supervisor, Configure},
            BeforeCommitContext, Service, TransactionContext,
        },
        DispatcherError, ExecutionError, InstanceDescriptor,
    },
};
use exonum_derive::ServiceFactory;
use exonum_merkledb::Snapshot;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crate::{
    api,
    blockchain::{BtcAnchoringSchema, Transactions},
    btc::{PrivateKey, PublicKey},
    config::Config,
    proto,
};

/// Btc anchoring service implementation for the Exonum blockchain.
#[derive(ServiceFactory)]
#[exonum(
    proto_sources = "proto",
    implements("Transactions", "Configure<Params = Config>")
)]
pub struct BtcAnchoringService;

impl std::fmt::Debug for BtcAnchoringService {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        f.debug_struct("BtcAnchoringService").finish()
    }
}

impl Service for BtcAnchoringService {
    fn initialize(
        &self,
        instance: InstanceDescriptor,
        fork: &Fork,
        params: Vec<u8>,
    ) -> Result<(), ExecutionError> {
        let config = Config::from_bytes(params.into())
            .and_then(ValidateInput::into_validated)
            .map_err(DispatcherError::malformed_arguments)?;

        let schema = BtcAnchoringSchema::new(instance.name, fork);
        // TODO remove this special case.
        if let Some(ref tx) = config.funding_transaction {
            schema.unspent_funding_transaction_entry().set(tx.clone());
        }
        schema.actual_config_entry().set(config);
        Ok(())
    }

    fn state_hash(&self, instance: InstanceDescriptor, snapshot: &dyn Snapshot) -> Vec<Hash> {
        BtcAnchoringSchema::new(instance.name, snapshot).state_hash()
    }

    fn before_commit(&self, context: BeforeCommitContext) {
        // Writes a hash of the latest block to the proof list index.
        let block_header_hash = CoreSchema::new(context.fork)
            .block_hashes_by_height()
            .last()
            .expect("An attempt to invoke execute during the genesis block initialization.");

        let schema = BtcAnchoringSchema::new(context.instance.name, context.fork);
        schema.anchored_blocks().push(block_header_hash);
    }

    fn wire_api(&self, builder: &mut ServiceApiBuilder) {
        api::wire(builder);
    }
}

impl Configure for BtcAnchoringService {
    type Params = Config;

    fn verify_config(
        &self,
        context: TransactionContext,
        params: Self::Params,
    ) -> Result<(), ExecutionError> {
        context
            .verify_caller(verify_caller_is_supervisor)
            .ok_or(DispatcherError::UnauthorizedCaller)?;

        params
            .validate()
            .map_err(DispatcherError::malformed_arguments)
    }

    fn apply_config(
        &self,
        context: TransactionContext,
        params: Self::Params,
    ) -> Result<(), ExecutionError> {
        let (_, fork) = context
            .verify_caller(verify_caller_is_supervisor)
            .ok_or(DispatcherError::UnauthorizedCaller)?;

        let schema = BtcAnchoringSchema::new(context.instance.name, fork);
        // TODO remove this special case.
        if let Some(ref tx) = params.funding_transaction {
            schema.unspent_funding_transaction_entry().set(tx.clone());
        }

        if schema.actual_configuration().anchoring_address() == params.anchoring_address() {
            // There are no changes in the anchoring address, so we just apply the config
            // immediately.
            schema.actual_config_entry().set(params);
        } else {
            // Set the config as the next one, which will become an actual after the transition
            // of the anchoring chain to the following address.
            schema.following_config_entry().set(params);
        }
        Ok(())
    }
}

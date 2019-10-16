// Copyright 2019 The Exonum Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSEccccc//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use exonum::{
    crypto::{Hash, PublicKey, SecretKey},
    helpers::Height,
    merkledb::ObjectHash,
    runtime::rust::Transaction,
};
use exonum_btc_anchoring::{
    api::{AnchoringChainLength, AnchoringProposalState, PrivateApi},
    blockchain::{AddFunds, BtcAnchoringSchema, SignInput},
    btc,
    config::Config,
    sync::{
        AnchoringChainUpdateTask, BitcoinRelay, ChainUpdateError, SyncWithBitcoinError,
        SyncWithBitcoinTask,
    },
    test_helpers::{AnchoringTestKit, ANCHORING_INSTANCE_ID, ANCHORING_INSTANCE_NAME},
};
use exonum_testkit::TestKitApi;
use futures::{Future, IntoFuture};

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
enum FakeRelayRequest {
    SendTransaction {
        request: btc::Transaction,
        response: btc::Sha256d,
    },
    TransactionConfirmations {
        request: btc::Sha256d,
        response: Option<u32>,
    },
}

impl FakeRelayRequest {
    fn into_send_transaction(self) -> (btc::Transaction, btc::Sha256d) {
        if let FakeRelayRequest::SendTransaction { request, response } = self {
            (request, response)
        } else {
            panic!(
                "Expected response for the `send_transaction` request. But got {:?}",
                self
            )
        }
    }

    fn into_transaction_confirmations(self) -> (btc::Sha256d, Option<u32>) {
        if let FakeRelayRequest::TransactionConfirmations { request, response } = self {
            (request, response)
        } else {
            panic!(
                "Expected response for the `transaction_confirmations` request. But got {:?}",
                self
            )
        }
    }
}

#[derive(Debug, Clone, Default)]
struct FakeBitcoinRelay {
    requests: Arc<Mutex<VecDeque<FakeRelayRequest>>>,
}

impl FakeBitcoinRelay {
    fn enqueue_requests(&self, requests: impl IntoIterator<Item = FakeRelayRequest>) {
        self.requests.lock().unwrap().extend(requests)
    }

    fn dequeue_request(&self) -> FakeRelayRequest {
        self.requests
            .lock()
            .unwrap()
            .pop_front()
            .expect("Expected relay request")
    }
}

impl Drop for FakeBitcoinRelay {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.requests.lock().unwrap().is_empty(),
                "Unhandled requests remained. {:?}",
                self
            );
        }
    }
}

impl BitcoinRelay for FakeBitcoinRelay {
    type Error = failure::Error;

    fn send_transaction(
        &self,
        transaction: &btc::Transaction,
    ) -> Result<btc::Sha256d, Self::Error> {
        let (expected_request, response) = self.dequeue_request().into_send_transaction();
        assert_eq!(&expected_request, transaction, "Unexpected data in request");
        Ok(response)
    }

    fn transaction_confirmations(&self, id: btc::Sha256d) -> Result<Option<u32>, Self::Error> {
        let (expected_request, response) = self.dequeue_request().into_transaction_confirmations();
        assert_eq!(expected_request, id, "Unexpected data in request");
        Ok(response)
    }
}

/// TODO Implement creating TestkitApi for an arbitrary TestNode. [ECR-3222]
#[derive(Debug)]
struct FakePrivateApi {
    service_keypair: (PublicKey, SecretKey),
    inner: TestKitApi,
}

impl FakePrivateApi {
    fn for_anchoring_node(testkit: &AnchoringTestKit, bitcoin_key: &btc::PublicKey) -> Self {
        let service_keypair = testkit
            .find_anchoring_node(bitcoin_key)
            .unwrap()
            .service_keypair();

        Self {
            service_keypair,
            inner: testkit.inner.api(),
        }
    }
}

impl PrivateApi for FakePrivateApi {
    type Error = exonum::api::Error;

    fn sign_input(
        &self,
        sign_input: SignInput,
    ) -> Box<dyn Future<Item = Hash, Error = Self::Error>> {
        let signed_tx = sign_input.sign(
            ANCHORING_INSTANCE_ID,
            self.service_keypair.0,
            &self.service_keypair.1,
        );
        let hash = signed_tx.object_hash();
        self.inner.send(signed_tx);
        Box::new(Ok(hash).into_future())
    }

    fn add_funds(
        &self,
        transaction: btc::Transaction,
    ) -> Box<dyn Future<Item = Hash, Error = Self::Error>> {
        let signed_tx = AddFunds { transaction }.sign(
            ANCHORING_INSTANCE_ID,
            self.service_keypair.0,
            &self.service_keypair.1,
        );
        let hash = signed_tx.object_hash();
        self.inner.send(signed_tx);
        Box::new(Ok(hash).into_future())
    }

    fn anchoring_proposal(&self) -> Result<AnchoringProposalState, Self::Error> {
        self.inner.anchoring_proposal()
    }

    fn config(&self) -> Result<Config, Self::Error> {
        self.inner.config()
    }

    fn transaction_with_index(&self, index: u64) -> Result<Option<btc::Transaction>, Self::Error> {
        self.inner.transaction_with_index(index)
    }

    fn transactions_count(&self) -> Result<AnchoringChainLength, Self::Error> {
        self.inner.transactions_count()
    }
}

fn anchoring_transaction_payload(testkit: &AnchoringTestKit, index: u64) -> Option<btc::Payload> {
    let snapshot = testkit.inner.snapshot();
    BtcAnchoringSchema::new(ANCHORING_INSTANCE_NAME, &snapshot)
        .anchoring_transactions_chain()
        .get(index)
        .map(|tx| tx.anchoring_payload().unwrap())
}

#[test]
fn chain_updater_normal() {
    let mut testkit = AnchoringTestKit::default();
    let anchoring_interval = testkit.actual_anchoring_config().anchoring_interval;
    // Commit several blocks.
    testkit
        .inner
        .create_blocks_until(Height(anchoring_interval));
    // Perform a several anchoring chain updates.
    for i in 0..2 {
        for keypair in testkit.anchoring_keypairs() {
            let api = FakePrivateApi::for_anchoring_node(&testkit, &keypair.0);
            AnchoringChainUpdateTask::new(vec![keypair], api)
                .process()
                .unwrap();
        }
        testkit.inner.create_block();
        // Make sure the anchoring proposal has been finalized.
        assert_eq!(
            anchoring_transaction_payload(&testkit, i)
                .unwrap()
                .block_height,
            Height(i * anchoring_interval)
        );
    }
}

#[test]
fn chain_updater_no_initial_funds() {
    let anchoring_interval = 5;
    let mut testkit = AnchoringTestKit::new(1, anchoring_interval);
    // Commit several blocks.
    testkit
        .inner
        .create_blocks_until(Height(anchoring_interval));
    // Try to perform anchoring chain update.
    let e = AnchoringChainUpdateTask::new(testkit.anchoring_keypairs(), testkit.inner.api())
        .process()
        .unwrap_err();

    match e {
        ChainUpdateError::NoInitialFunds => {}
        e => panic!("Unexpected error occurred: {:?}", e),
    }
}

#[test]
fn chain_updater_insufficient_funds() {
    let anchoring_interval = 5;
    let mut testkit = AnchoringTestKit::new(1, anchoring_interval);

    // Add an initial funding transaction to enable anchoring.
    testkit
        .inner
        .create_block_with_transactions(testkit.create_funding_confirmation_txs(200).0);

    // Commit several blocks.
    testkit
        .inner
        .create_blocks_until(Height(anchoring_interval));
    // Try to perform anchoring chain update.
    let e = AnchoringChainUpdateTask::new(testkit.anchoring_keypairs(), testkit.inner.api())
        .process()
        .unwrap_err();

    match e {
        ChainUpdateError::InsufficientFunds { balance, total_fee } => {
            assert_eq!(balance, 200);
            assert_eq!(total_fee, 1530);
        }
        e => panic!("Unexpected error occurred: {:?}", e),
    }
}

#[test]
fn sync_with_bitcoin_normal() {
    let mut testkit = AnchoringTestKit::default();
    let anchoring_interval = testkit.actual_anchoring_config().anchoring_interval;
    // Create a several anchoring transactions
    for i in 0..2 {
        testkit
            .inner
            .create_blocks_until(Height(anchoring_interval * i));

        testkit
            .inner
            .create_block_with_transactions(testkit.create_signature_txs().into_iter().flatten());
    }

    // Check that sync with bitcoin works as expected.
    let snapshot = testkit.inner.snapshot();
    let anchoring_schema = BtcAnchoringSchema::new(ANCHORING_INSTANCE_NAME, &snapshot);
    let tx_chain = anchoring_schema.anchoring_transactions_chain();

    let fake_relay = FakeBitcoinRelay::default();
    let sync = SyncWithBitcoinTask::new(fake_relay.clone(), testkit.inner.api());
    // Send first anchoring transaction.
    fake_relay.enqueue_requests(vec![
        // Relay should see that we have only a funding transaction confirmed.
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(1).unwrap().id(),
            response: None,
        },
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(0).unwrap().id(),
            response: None,
        },
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(0).unwrap().prev_tx_id(),
            response: Some(10),
        },
        // Ensure that relay sends first anchoring transaction to the Bitcoin network.
        FakeRelayRequest::SendTransaction {
            request: tx_chain.get(0).unwrap(),
            response: tx_chain.get(0).unwrap().id(),
        },
    ]);
    let latest_committed_tx_index = sync
        .process(None)
        .unwrap()
        .expect("Transaction should be committed");
    assert_eq!(latest_committed_tx_index, 0);
    // Send second anchoring transaction.
    fake_relay.enqueue_requests(vec![
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(1).unwrap().id(),
            response: None,
        },
        FakeRelayRequest::SendTransaction {
            request: tx_chain.get(1).unwrap(),
            response: tx_chain.get(1).unwrap().id(),
        },
    ]);
    let latest_committed_tx_index = sync
        .process(Some(1))
        .unwrap()
        .expect("Transaction should be committed");
    assert_eq!(latest_committed_tx_index, 1);
}

#[test]
fn sync_with_bitcoin_empty_chain() {
    let testkit = AnchoringTestKit::default();
    assert!(
        SyncWithBitcoinTask::new(FakeBitcoinRelay::default(), testkit.inner.api())
            .process(None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn sync_with_bitcoin_err_unconfirmed_funding_tx() {
    let mut testkit = AnchoringTestKit::default();
    // Establish anchoring transactions chain.
    testkit
        .inner
        .create_block_with_transactions(testkit.create_signature_txs().into_iter().flatten());
    // Check that synchronization will cause an error if the funding transaction was not confirmed.
    let snapshot = testkit.inner.snapshot();
    let anchoring_schema = BtcAnchoringSchema::new(ANCHORING_INSTANCE_NAME, &snapshot);
    let tx_chain = anchoring_schema.anchoring_transactions_chain();

    let fake_relay = FakeBitcoinRelay::default();
    let sync = SyncWithBitcoinTask::new(fake_relay.clone(), testkit.inner.api());
    fake_relay.enqueue_requests(vec![
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(0).unwrap().id(),
            response: None,
        },
        FakeRelayRequest::TransactionConfirmations {
            request: tx_chain.get(0).unwrap().prev_tx_id(),
            response: None,
        },
    ]);

    let e = sync.process(None).unwrap_err();
    match e {
        SyncWithBitcoinError::UnconfirmedFundingTransaction(hash) => {
            assert_eq!(hash, tx_chain.get(0).unwrap().prev_tx_id())
        }
        e => panic!("Unexpected error occurred: {:?}", e),
    }
}
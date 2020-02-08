/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This Source Code Form is "Incompatible With Secondary Licenses", as
 * defined by the Mozilla Public License, v. 2.0. */

//! Low-level structured concurrency.
//!
//! This module gives you everything you need to spawn a task but does not do the actual spawning.

pub mod signals;

use self::signals::*;
use crate::{ChildSignals, ParentSignals, RawScopedSpawn, SignalReceiver, SignalSender};
use futures::channel::oneshot;
use futures::future;
use futures::pin_mut;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

#[cfg(debug_assertions)]
const EXPECT_SPAWNER_NO_OWNER: &str =
    "Remote spawner is still owned after we dropped the its owner future. \
     It's considered a programming error to send the spawner anywhere \
     other than the task owning the spawner. Please check that the \
     spawner isn't sent to another task or thread, or stored in any \
     global variable. This check does not cover all possible errors, and \
     is only performed if debug assertions are enabled.";

#[cfg(debug_assertions)]
const EXPECT_SPAWNER_NO_CONTENTION: &str =
    "Another thread is using the remote spawner to spawn a task. \
     It's considered a programming error to send the spawner anywhere \
     other than the task owning the spawner. Please check that the \
     spawner isn't sent to another task or thread, or stored in any \
     global variable. This check does not cover all possible errors, and \
     is only performed if debug assertions are enabled.";

/// A scope for a child task.
pub struct RemoteScope {
    spawner: Option<RemoteSpawner>,
}

/// A spawner.
///
/// This type is returned by `RemoteScope::spawner`.
///
/// This type implements `RawScopedSpawn`.
#[derive(Clone)]
pub struct RemoteSpawner {
    child_cancel_senders: Arc<Mutex<HashMap<usize, oneshot::Sender<()>>>>,
    child_done_receivers: Arc<Mutex<HashMap<usize, oneshot::Receiver<()>>>>,
    #[cfg(debug_assertions)]
    owner_test: Arc<Mutex<()>>, // Used to track if the spawner was sent anywhere else
}

impl RemoteScope {
    /// Constructs a new `RemoteScope`.
    pub fn new() -> Self {
        Self { spawner: None }
    }

    /// Returns a spawner for the scoped child task.
    ///
    /// It may be cheaper if this method is not called.
    pub fn spawner(&mut self) -> RemoteSpawner {
        match &self.spawner {
            Some(spawner) => spawner.clone(),
            None => {
                let spawner = RemoteSpawner::new();
                self.spawner = Some(spawner.clone());
                spawner
            }
        }
    }

    /// Wraps `signal`, `fut`, and `done` into a future suitable to be run in a new task.
    ///
    /// The returned future will handle all signals for structured concurrency. It will also handle
    /// termination as described in the library overview.
    pub fn wrap<ParentCancelReceiver, ParentDoneSender, Fut, Done>(
        self,
        signal: ChildSignals<ParentCancelReceiver, ParentDoneSender>,
        fut: Fut,
        done: Done,
    ) -> impl Future<Output = ()>
    where
        ParentCancelReceiver: SignalReceiver,
        ParentDoneSender: SignalSender,
        Fut: Future<Output = ()>,
        Done: FnOnce(),
    {
        struct DropOrder<ParentCancelReceiver, ParentDoneSender, Fut, Done> {
            spawner: Option<RemoteSpawner>,
            fut: Fut,
            done: Done,
            parent_cancel_receiver: ParentCancelReceiver,
            parent_done_sender: ParentDoneSender,
        }

        let data = DropOrder {
            spawner: self.spawner,
            fut,
            done,
            parent_cancel_receiver: signal.cancel_receiver,
            parent_done_sender: signal.done_sender,
        };

        async move {
            {
                {
                    let parent_cancel_receiver = data.parent_cancel_receiver;
                    pin_mut!(parent_cancel_receiver);
                    let fut = data.fut;
                    pin_mut!(fut);

                    future::select(parent_cancel_receiver, fut).await;
                }

                if let Some(spawner) = data.spawner {
                    #[cfg(debug_assertions)]
                    {
                        // It's not allowed to send the spawner anywhere else
                        Arc::try_unwrap(spawner.owner_test).expect(EXPECT_SPAWNER_NO_OWNER);
                    }
                    {
                        // Drop child cancel senders
                        *spawner.child_cancel_senders.lock().unwrap() = HashMap::new();
                    }
                    let receivers = {
                        std::mem::replace(
                            &mut *spawner.child_done_receivers.lock().unwrap(),
                            HashMap::new(),
                        )
                    };
                    for receiver in receivers {
                        let _ = receiver.1.await;
                    }
                }
                {
                    let done = data.done;
                    done();
                }
            }

            drop(data.parent_done_sender);
        }
    }
}

impl Default for RemoteScope {
    fn default() -> RemoteScope {
        Self::new()
    }
}

impl RemoteSpawner {
    fn new() -> Self {
        Self {
            child_cancel_senders: Arc::new(Mutex::new(HashMap::new())),
            child_done_receivers: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(debug_assertions)]
            owner_test: Arc::new(Mutex::new(())),
        }
    }
}

impl RawScopedSpawn for RemoteSpawner {
    type CancelReceiver = RemoteCancelReceiver;
    type DoneSender = RemoteDoneSender;

    type CancelSenderWithSignal = RemoteCancelSenderWithSignal;
    type DoneReceiverWithSignal = RemoteDoneReceiverWithSignal;
    type CancelReceiverWithSignal = RemoteCancelReceiverWithSignal;
    type DoneSenderWithSignal = RemoteDoneSenderWithSignal;

    fn spawn_raw(&self) -> ChildSignals<Self::CancelReceiver, Self::DoneSender> {
        let (cancel_sender, cancel_receiver) = oneshot::channel();
        let cancel_sender_id = Box::pin(0u8);
        let (done_sender, done_receiver) = oneshot::channel();
        let done_receiver_id = Box::pin(0u8);

        #[cfg(debug_assertions)]
        let _guard = self
            .owner_test
            .try_lock()
            .expect(EXPECT_SPAWNER_NO_CONTENTION);

        self.child_cancel_senders
            .lock()
            .unwrap()
            .insert((&*cancel_sender_id) as *const u8 as usize, cancel_sender);
        self.child_done_receivers
            .lock()
            .unwrap()
            .insert((&*done_receiver_id) as *const u8 as usize, done_receiver);

        ChildSignals {
            cancel_receiver: RemoteCancelReceiver {
                receiver: cancel_receiver,
                sender_id: cancel_sender_id,
                senders: self.child_cancel_senders.clone(),
            },
            done_sender: RemoteDoneSender {
                _sender: done_sender,
                receiver_id: done_receiver_id,
                receivers: self.child_done_receivers.clone(),
            },
        }
    }

    fn spawn_raw_with_signal(
        &self,
    ) -> super::ParentChildSignals<
        Self::CancelSenderWithSignal,
        Self::DoneReceiverWithSignal,
        Self::CancelReceiverWithSignal,
        Self::DoneSenderWithSignal,
    > {
        let (cancel_sender_root, cancel_receiver_root) = oneshot::channel();
        let cancel_sender_root_id = Box::pin(0u8);
        let (cancel_sender_leaf, cancel_receiver_leaf) = oneshot::channel();
        let (done_sender_root, done_receiver_root) = oneshot::channel();
        let done_receiver_root_id = Box::pin(0u8);
        let (done_sender_leaf, done_receiver_leaf) = oneshot::channel();

        #[cfg(debug_assertions)]
        let _guard = self
            .owner_test
            .try_lock()
            .expect(EXPECT_SPAWNER_NO_CONTENTION);

        self.child_cancel_senders.lock().unwrap().insert(
            (&*cancel_sender_root_id) as *const u8 as usize,
            cancel_sender_root,
        );
        self.child_done_receivers.lock().unwrap().insert(
            (&*done_receiver_root_id) as *const u8 as usize,
            done_receiver_root,
        );

        (
            ParentSignals {
                cancel_sender: RemoteCancelSenderWithSignal {
                    _sender: cancel_sender_leaf,
                },
                done_receiver: RemoteDoneReceiverWithSignal {
                    receiver: done_receiver_leaf,
                },
            },
            ChildSignals {
                cancel_receiver: RemoteCancelReceiverWithSignal {
                    receiver_root: cancel_receiver_root,
                    receiver_leaf: cancel_receiver_leaf,
                    sender_id: cancel_sender_root_id,
                    senders: self.child_cancel_senders.clone(),
                },
                done_sender: RemoteDoneSenderWithSignal {
                    _sender_root: done_sender_root,
                    _sender_leaf: done_sender_leaf,
                    receiver_id: done_receiver_root_id,
                    receivers: self.child_done_receivers.clone(),
                },
            },
        )
    }
}

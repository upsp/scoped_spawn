/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This Source Code Form is "Incompatible With Secondary Licenses", as
 * defined by the Mozilla Public License, v. 2.0. */

//! Full structured concurrency for asynchronous programming.
//!
//! # Structured Concurrency
//!
//! In structured concurrency, each asynchronous task only runs within a certain
//! scope. This scope is created by the task's parent and cannot exceed the
//! parent's scope.
//!
//! At each point in time, tasks created within structured concurrency form a tree.
//! If a node is alive, all the nodes on its path to the root are also alive. In
//! other words, no node can outlive its parent.
//!
//! This library provides a strong guarantee of structured concurrency: a child
//! task must completely exit and release all resources (except for the resources
//! to notify its parent of its termination, which is handled by this library)
//! before we consider it terminated.
//!
//! # API Overview
//!
//! This library provides the `ScopedSpawn` trait from which you can spawn new
//! tasks. The spawned tasks become children of the current task and will be
//! terminated when the current task begins to terminate. The API also provides
//! methods with which you could terminate a child task earlier.
//!
//! Termination initiated outside the task to be terminated is also called
//! cancellation.
//!
//! The `ScopedSpawn` trait is implemented by `ScopedSpawner`. To create a
//! `ScopedSpawner`, pass it an object that implements `Spawn`, which you can
//! trivially implement for all known executors.
//!
//! Any code that wishes to accept a spawner should accept the `ScopedSpawn` trait
//! instead of `ScopedSpawner`.
//!
//! # Termination of a Task
//!
//! The termination process has several phases.
//!
//! 1. Termination begins upon the completion of the task's future or the receipt
//!    of cancel signal from the parent, whichever comes first.
//! 2. The task's future is immediately dropped.
//! 3. The task in turn sends cancel signals to its children.
//! 4. The task asynchronously waits for its children to terminate.
//! 5. The `done` function is called in the task. For details see the documentation
//!    for `ScopedSpawn`.
//! 6. Finally, the task signals its termination to its parent through the "done"
//!    signal. For details see the documentation for `ParentSignals` and
//!    `ChildSignals`.
//!
//! # Low-level API
//!
//! A low-level `remote_scope` API is also provided. It gives you everything you
//! need to spawn a task but does not do the actual spawning.
//!
//! # Example
//!
//! The following example demonstrates `ScopedSpawn` when using Tokio.
//!
//! ```rust
//! #[tokio::main]
//! async fn main() {
//!     use scoped_spawn::{ScopedSpawn, ScopedSpawner};
//!
//!     let spawn = TokioDefaultSpawner::new();
//!     let spawn = ScopedSpawner::new(spawn);
//!     let signal = spawn
//!         .spawn_with_signal(
//!             |spawn| async {
//!                 // Here `spawn` is the child's spawner. Do not give it to anyone else!
//!                 // And do not try to use the parent's spawner because it would break structured
//!                 // concurrency.
//!
//!                 // We could spawn nested children here, but for the demo we don't.
//!                 drop(spawn);
//!
//!                 eprintln!("I'm alive!");
//!                 tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
//!                 eprintln!("I'm still alive!"); // Nope.
//!             },
//!             || (),
//!         )
//!         .unwrap();
//!
//!     tokio::time::delay_for(std::time::Duration::from_secs(1)).await;
//!
//!     drop(signal.cancel_sender); // Cancel the task by dropping.
//!     signal.done_receiver.await; // Do this if you want to wait.
//!                                 // When the await returns, the future of the spawned task is
//!                                 // guaranteed dropped.
//!     eprintln!("Task terminated.");
//! }
//!
//! // Just some chores to turn the Tokio spawner into a `Spawn`.
//! #[derive(Clone)]
//! struct TokioDefaultSpawner {}
//!
//! impl TokioDefaultSpawner {
//!     fn new() -> Self {
//!         Self {}
//!     }
//! }
//!
//! impl futures::task::Spawn for TokioDefaultSpawner {
//!     fn spawn_obj(
//!         &self,
//!         future: futures::future::FutureObj<'static, ()>,
//!     ) -> Result<(), futures::task::SpawnError> {
//!         tokio::spawn(future);
//!         Ok(())
//!     }
//!
//!     fn status(&self) -> Result<(), futures::task::SpawnError> {
//!         Ok(())
//!     }
//! }
//! ```

pub mod remote_scope;

use self::remote_scope::{RemoteScope, RemoteSpawner};
use futures::task::{self, SpawnError, SpawnExt};
use std::future::Future;

/// An object that sends a signal when dropped.
pub trait SignalSender {} // TODO: Allow "forgetting" the sender.

/// A future that resolves when receiving a signal.
pub trait SignalReceiver: Future<Output = ()> {}

/// Signal sender and receiver for parent tasks.
pub struct ParentSignals<CancelSender: SignalSender, DoneReceiver: SignalReceiver> {
    /// Signal sender to send cancel signal to child task.
    pub cancel_sender: CancelSender,
    /// Signal receiver to receive done signal from child task.
    pub done_receiver: DoneReceiver,
}

/// Signal sender and receiver for children tasks.
pub struct ChildSignals<CancelReceiver: SignalReceiver, DoneSender: SignalSender> {
    /// Signal receiver to receive cancel signal from parent task.
    pub cancel_receiver: CancelReceiver,
    /// Signal sender to send done signal to parent task.
    pub done_sender: DoneSender,
}

type ParentChildSignals<CancelSender, DoneReceiver, CancelReceiver, DoneSender> = (
    ParentSignals<CancelSender, DoneReceiver>,
    ChildSignals<CancelReceiver, DoneSender>,
);

/// Allows for spawning tasks in a scope. Implementors should provide structured concurrency.
///
/// # Spawning a task
///
/// When spawning a task, you can choose to allow the spawned task to spawn more tasks on its own.
/// In this case, use `spawn` and `spawn_with_signal`, whose input is a function that takes a
/// spawner for the child task and returns a future. If you do not need a spawner for the child
/// task, use `spawn_future` and `spawn_future_with_signal`, in which case the spawned object is
/// only a future.
pub trait ScopedSpawn: Clone + Send + Sync {
    /// The type of the signal sender for parents to initiate task termination.
    type CancelSender: SignalSender + Send;
    /// The type of the signal receiver for parents to wait for task termination.
    type DoneReceiver: SignalReceiver + Send;

    /// The type of the signal sender for parents to initiate task termination, when the spawned
    /// object is only a future.
    type FutureCancelSender: SignalSender + Send;
    /// The type of the signal receiver for parents to wait for task termination, when the spawned
    /// object is only a future.
    type FutureDoneReceiver: SignalReceiver + Send;

    /// The type of the raw spawner.
    type Raw: RawScopedSpawn;

    /// Spawns a task to run the future returned by `fun`. The spawned task will call `done` after
    /// all its children terminate.
    fn spawn<Fut, Fun, Done>(&self, fun: Fun, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static;

    /// Spawns a task to run the future returned by `fun`. The spawned task will call `done` after
    /// all its children terminate.
    ///
    /// The `ParentSignals` returned by this method can be used to cancel the task and wait for
    /// task termination.
    fn spawn_with_signal<Fut, Fun, Done>(
        &self,
        fun: Fun,
        done: Done,
    ) -> Result<ParentSignals<Self::CancelSender, Self::DoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static;

    /// Spawns a task to run `fut`. The spawned task will call `done` after all its children
    /// terminate.
    fn spawn_future<Fut, Done>(&self, fut: Fut, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static;

    /// Spawns a task to run `fut`. The spawned task will call `done` after all its children
    /// terminate.
    ///
    /// The `ParentSignals` returned by this method can be used to cancel the task and wait for
    /// task termination.
    fn spawn_future_with_signal<Fut, Done>(
        &self,
        fut: Fut,
        done: Done,
    ) -> Result<ParentSignals<Self::FutureCancelSender, Self::FutureDoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static;

    /// Returns a reference to the raw spawner.
    fn as_raw(&self) -> &Self::Raw;

    /// Converts `self` into the raw spawner.
    fn into_raw(self) -> Self::Raw;
}

/// The low-level trait for spawning tasks in a scope.
pub trait RawScopedSpawn {
    /// The type of the signal receiver for children to receive cancellation signals.
    type CancelReceiver: SignalReceiver + Send;
    /// The type of the signal sender for children to signal their termination.
    type DoneSender: SignalSender + Send;

    /// The type of the signal sender for parents to initiate task termination, when using
    /// `spawn_raw_with_signal`.
    type CancelSenderWithSignal: SignalSender + Send;
    /// The type of the signal receiver for parents to wait for task termination, when using
    /// `spawn_raw_with_signal`.
    type DoneReceiverWithSignal: SignalReceiver + Send;
    /// The type of the signal receiver for children to receive cancellation signals, when using
    /// `spawn_raw_with_signal`.
    type CancelReceiverWithSignal: SignalReceiver + Send;
    /// The type of the signal sender for children to signal their termination, when using
    /// `spawn_raw_with_signal`.
    type DoneSenderWithSignal: SignalSender + Send;

    /// Creates signals to be used by a child task.
    ///
    /// The child task should start termination whenever a signal is sent to `CancelReceiver`. For
    /// example, the task could poll `CancelReceiver` at every yield point.
    ///
    /// The child task should send a signal from `DoneSender` __after__ its own children tasks are
    /// terminated.
    fn spawn_raw(&self) -> ChildSignals<Self::CancelReceiver, Self::DoneSender>;

    /// Creates signals to be used by a child task.
    ///
    /// `CancelSenderWithSignal` sends a signal to `CancelReceiverWithSignal`, and
    /// `DoneReceiverWithSignal` receives the signal from `DoneSenderWithSignal`. These signals
    /// exist in addition to the signals managed by this library for structured concurrency.
    ///
    /// The child task should start termination whenever a signal is sent to
    /// `CancelReceiverWithSignal`. For example, the task could poll `CancelReceiverWithSignal` at
    /// every yield point.
    ///
    /// The child task should send a signal from `DoneSenderWithSignal` __after__ its own children
    /// tasks are terminated.
    fn spawn_raw_with_signal(
        &self,
    ) -> ParentChildSignals<
        Self::CancelSenderWithSignal,
        Self::DoneReceiverWithSignal,
        Self::CancelReceiverWithSignal,
        Self::DoneSenderWithSignal,
    >;
}

/// A scoped spawner.
///
/// This type implements `ScopedSpawn`.
#[derive(Clone)]
pub struct ScopedSpawner<Spawn: task::Spawn + Clone + Send + Sync> {
    spawner: Spawn,
    remote_spawner: RemoteSpawner,
}

impl<Spawn: task::Spawn + Clone + Send + Sync> ScopedSpawner<Spawn> {
    /// Constructs a new scoped spawner from a traditional spawner.
    pub fn new(spawner: Spawn) -> Self {
        Self {
            spawner,
            remote_spawner: RemoteScope::new().spawner(),
        }
    }
}

impl<Spawn: task::Spawn + Clone + Send + Sync> ScopedSpawn for ScopedSpawner<Spawn> {
    type CancelSender = remote_scope::signals::RemoteCancelSenderWithSignal;
    type DoneReceiver = remote_scope::signals::RemoteDoneReceiverWithSignal;

    type FutureCancelSender = remote_scope::signals::RemoteCancelSenderWithSignal;
    type FutureDoneReceiver = remote_scope::signals::RemoteDoneReceiverWithSignal;

    type Raw = RemoteSpawner;

    fn spawn<Fut, Fun, Done>(&self, fun: Fun, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static,
    {
        let cancel = self.remote_spawner.spawn_raw();
        let mut scope = RemoteScope::new();
        let child_spawn = scope.spawner();
        let child_spawn = ScopedSpawner {
            spawner: self.spawner.clone(),
            remote_spawner: child_spawn,
        };
        let fut = fun(child_spawn);
        let wrap = scope.wrap(cancel, fut, done);
        self.spawner.spawn(wrap)
    }

    fn spawn_with_signal<Fut, Fun, Done>(
        &self,
        fun: Fun,
        done: Done,
    ) -> Result<ParentSignals<Self::CancelSender, Self::DoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static,
    {
        let (cancel_source, cancel) = self.remote_spawner.spawn_raw_with_signal();
        let mut scope = RemoteScope::new();
        let child_spawn = scope.spawner();
        let child_spawn = ScopedSpawner {
            spawner: self.spawner.clone(),
            remote_spawner: child_spawn,
        };
        let fut = fun(child_spawn);
        let wrap = scope.wrap(cancel, fut, done);
        self.spawner.spawn(wrap).map(|_| cancel_source)
    }

    fn spawn_future<Fut, Done>(&self, fut: Fut, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static,
    {
        let cancel = self.remote_spawner.spawn_raw();
        let scope = RemoteScope::new();
        let wrap = scope.wrap(cancel, fut, done);
        self.spawner.spawn(wrap)
    }

    fn spawn_future_with_signal<Fut, Done>(
        &self,
        fut: Fut,
        done: Done,
    ) -> Result<ParentSignals<Self::FutureCancelSender, Self::FutureDoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static,
    {
        let (cancel_source, cancel) = self.remote_spawner.spawn_raw_with_signal();
        let scope = RemoteScope::new();
        let wrap = scope.wrap(cancel, fut, done);
        self.spawner.spawn(wrap).map(|_| cancel_source)
    }

    fn as_raw(&self) -> &Self::Raw {
        &self.remote_spawner
    }

    fn into_raw(self) -> Self::Raw {
        self.remote_spawner
    }
}

/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This Source Code Form is "Incompatible With Secondary Licenses", as
 * defined by the Mozilla Public License, v. 2.0. */

pub mod remote_scope;

use self::remote_scope::{RemoteScope, RemoteSpawner};
use futures::task::{self, SpawnError, SpawnExt};
use std::future::Future;

pub trait SignalSender {}

pub trait SignalReceiver: Future<Output = ()> {}

pub struct ParentSignals<CancelSender: SignalSender, DoneReceiver: SignalReceiver> {
    pub cancel_sender: CancelSender,
    pub done_receiver: DoneReceiver,
}

pub struct ChildSignals<CancelReceiver: SignalReceiver, DoneSender: SignalSender> {
    pub cancel_receiver: CancelReceiver,
    pub done_sender: DoneSender,
}

type ParentChildSignals<CancelSender, DoneReceiver, CancelReceiver, DoneSender> = (
    ParentSignals<CancelSender, DoneReceiver>,
    ChildSignals<CancelReceiver, DoneSender>,
);

pub trait ScopedSpawn: Clone + Send + Sync {
    type CancelSender: SignalSender + Send;
    type DoneReceiver: SignalReceiver + Send;

    type FutureCancelSender: SignalSender + Send;
    type FutureDoneReceiver: SignalReceiver + Send;

    type Raw: RawScopedSpawn;

    fn spawn<Fut, Fun, Done>(&self, fun: Fun, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static;

    fn spawn_with_signal<Fut, Fun, Done>(
        &self,
        fun: Fun,
        done: Done,
    ) -> Result<ParentSignals<Self::CancelSender, Self::DoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Fun: FnOnce(Self) -> Fut,
        Done: FnOnce() + Send + 'static;

    fn spawn_future<Fut, Done>(&self, fut: Fut, done: Done) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static;

    fn spawn_future_with_signal<Fut, Done>(
        &self,
        fut: Fut,
        done: Done,
    ) -> Result<ParentSignals<Self::FutureCancelSender, Self::FutureDoneReceiver>, SpawnError>
    where
        Fut: Future<Output = ()> + Send + 'static,
        Done: FnOnce() + Send + 'static;

    fn as_raw(&self) -> &Self::Raw;

    fn into_raw(self) -> Self::Raw;
}

pub trait RawScopedSpawn {
    type CancelReceiver: SignalReceiver + Send;
    type DoneSender: SignalSender + Send;

    type CancelSenderWithSignal: SignalSender + Send;
    type DoneReceiverWithSignal: SignalReceiver + Send;
    type CancelReceiverWithSignal: SignalReceiver + Send;
    type DoneSenderWithSignal: SignalSender + Send;

    fn spawn_raw(&self) -> ChildSignals<Self::CancelReceiver, Self::DoneSender>;

    fn spawn_raw_with_signal(
        &self,
    ) -> ParentChildSignals<
        Self::CancelSenderWithSignal,
        Self::DoneReceiverWithSignal,
        Self::CancelReceiverWithSignal,
        Self::DoneSenderWithSignal,
    >;
}

#[derive(Clone)]
pub struct ScopedSpawner<Spawn: task::Spawn + Clone + Send + Sync> {
    spawner: Spawn,
    remote_spawner: RemoteSpawner,
}

impl<Spawn: task::Spawn + Clone + Send + Sync> ScopedSpawner<Spawn> {
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

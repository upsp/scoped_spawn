/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This Source Code Form is "Incompatible With Secondary Licenses", as
 * defined by the Mozilla Public License, v. 2.0. */

//! Signals used by this library.
//!
//! See the documentation for `SignalSender` and `SignalReceiver`.

use crate::{SignalReceiver, SignalSender};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Remote cancel receiver.
pub struct RemoteCancelReceiver {
    pub(crate) receiver: oneshot::Receiver<()>,
    pub(crate) sender_id: Pin<Box<u8>>,
    pub(crate) senders: Arc<Mutex<HashMap<usize, oneshot::Sender<()>>>>,
}

impl Future for RemoteCancelReceiver {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx).map(|_| ())
    }
}

impl Drop for RemoteCancelReceiver {
    fn drop(&mut self) {
        let mut senders = self.senders.lock().unwrap();
        senders.remove(&((&*self.sender_id) as *const u8 as usize));
        senders.shrink_to_fit();
    }
}

impl SignalReceiver for RemoteCancelReceiver {}

/// Remote done sender.
pub struct RemoteDoneSender {
    pub(crate) _sender: oneshot::Sender<()>,
    pub(crate) receiver_id: Pin<Box<u8>>,
    pub(crate) receivers: Arc<Mutex<HashMap<usize, oneshot::Receiver<()>>>>,
}

impl Drop for RemoteDoneSender {
    fn drop(&mut self) {
        let mut receivers = self.receivers.lock().unwrap();
        receivers.remove(&((&*self.receiver_id) as *const u8 as usize));
        receivers.shrink_to_fit();
    }
}

impl SignalSender for RemoteDoneSender {}

/// Remote cancel sender for parent to send cancel signal.
pub struct RemoteCancelSenderWithSignal {
    pub(crate) _sender: oneshot::Sender<()>,
}

impl SignalSender for RemoteCancelSenderWithSignal {}

/// Remote cancel receiver, which also receives cancel signal from parent.
pub struct RemoteCancelReceiverWithSignal {
    pub(crate) receiver_root: oneshot::Receiver<()>,
    pub(crate) receiver_leaf: oneshot::Receiver<()>,
    pub(crate) sender_id: Pin<Box<u8>>,
    pub(crate) senders: Arc<Mutex<HashMap<usize, oneshot::Sender<()>>>>,
}

impl Future for RemoteCancelReceiverWithSignal {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver_root).poll(cx) {
            Poll::Pending => Pin::new(&mut self.receiver_leaf).poll(cx).map(|_| ()),
            Poll::Ready(_) => Poll::Ready(()),
        }
    }
}

impl Drop for RemoteCancelReceiverWithSignal {
    fn drop(&mut self) {
        let mut senders = self.senders.lock().unwrap();
        senders.remove(&((&*self.sender_id) as *const u8 as usize));
        senders.shrink_to_fit();
    }
}

impl SignalReceiver for RemoteCancelReceiverWithSignal {}

/// Remote done sender, which also sends done signal to parent.
pub struct RemoteDoneSenderWithSignal {
    pub(crate) _sender_root: oneshot::Sender<()>,
    pub(crate) _sender_leaf: oneshot::Sender<()>,
    pub(crate) receiver_id: Pin<Box<u8>>,
    pub(crate) receivers: Arc<Mutex<HashMap<usize, oneshot::Receiver<()>>>>,
}

impl Drop for RemoteDoneSenderWithSignal {
    fn drop(&mut self) {
        let mut receivers = self.receivers.lock().unwrap();
        receivers.remove(&((&*self.receiver_id) as *const u8 as usize));
        receivers.shrink_to_fit();
    }
}

impl SignalSender for RemoteDoneSenderWithSignal {}

/// Remote done receiver for parent to receive done signal.
pub struct RemoteDoneReceiverWithSignal {
    pub(crate) receiver: oneshot::Receiver<()>,
}

impl Future for RemoteDoneReceiverWithSignal {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx).map(|_| ())
    }
}

impl SignalReceiver for RemoteDoneReceiverWithSignal {}

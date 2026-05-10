#![forbid(unsafe_code)]

use core::ops::{Deref, DerefMut};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Platform-neutral access to shared ownership, interior mutability, and channels.
pub trait Platform: 'static {
    type Shared<T: 'static>: Clone + Deref<Target = T>;
    type RwLock<T: 'static>;
    type ReadGuard<'a, T: 'static>: Deref<Target = T>
    where
        Self: 'a,
        T: 'a;
    type WriteGuard<'a, T: 'static>: DerefMut<Target = T>
    where
        Self: 'a,
        T: 'a;
    type Sender<T: Clone + 'static>: Clone + PlatformSender<T>;
    type Receiver<T: Clone + 'static>: PlatformReceiver<T>;

    fn shared<T: 'static>(value: T) -> Self::Shared<T>;
    fn new_rwlock<T: 'static>(value: T) -> Self::RwLock<T>;
    fn read<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::ReadGuard<'a, T>;
    fn write<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::WriteGuard<'a, T>;
    fn channel<T: Clone + 'static>(capacity: usize) -> (Self::Sender<T>, Self::Receiver<T>);
}

/// Sender side of a bounded mailbox channel.
pub trait PlatformSender<T> {
    type Error;

    fn send(&self, value: T) -> Result<(), Self::Error>;
}

/// Receiver side of a bounded mailbox channel.
pub trait PlatformReceiver<T> {
    fn try_recv(&mut self) -> Option<T>;
}

/// Marker for native multi-threaded execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativePlatform;

/// Marker for single-threaded WASM execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct WasmPlatform;

/// Phase 0 error type for mailbox sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendError;

#[derive(Debug)]
struct MailboxState<T> {
    capacity: usize,
    start_sequence: u64,
    next_sequence: u64,
    items: VecDeque<(u64, T)>,
}

impl<T> MailboxState<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            start_sequence: 0,
            next_sequence: 0,
            items: VecDeque::new(),
        }
    }

    fn push(&mut self, value: T) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.items.push_back((sequence, value));

        while self.items.len() > self.capacity {
            let _ = self.items.pop_front();
            self.start_sequence = self.start_sequence.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeSender<T> {
    mailbox: Arc<NativeMailbox<T>>,
}

#[derive(Debug)]
pub struct NativeReceiver<T> {
    mailbox: Arc<NativeMailbox<T>>,
    next_sequence: u64,
}

#[derive(Debug)]
struct NativeMailbox<T> {
    state: Mutex<MailboxState<T>>,
    ready: Condvar,
}

impl<T> NativeMailbox<T> {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(MailboxState::new(capacity)),
            ready: Condvar::new(),
        }
    }

    fn wake(&self) {
        self.ready.notify_all();
    }
}

#[derive(Debug, Clone)]
pub struct NativeWakeHandle<T> {
    mailbox: Arc<NativeMailbox<T>>,
}

impl<T> NativeWakeHandle<T> {
    pub fn wake(&self) {
        self.mailbox.wake();
    }
}

impl<T: Clone> PlatformSender<T> for NativeSender<T> {
    type Error = SendError;

    fn send(&self, value: T) -> Result<(), Self::Error> {
        let mut state = match self.mailbox.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.push(value);
        self.mailbox.ready.notify_all();
        Ok(())
    }
}

impl<T: Clone> PlatformReceiver<T> for NativeReceiver<T> {
    fn try_recv(&mut self) -> Option<T> {
        let state = match self.mailbox.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        receive_from_state(state.start_sequence, &state.items, &mut self.next_sequence)
    }
}

impl<T: Clone> NativeReceiver<T> {
    pub fn wake_handle(&self) -> NativeWakeHandle<T> {
        NativeWakeHandle {
            mailbox: Arc::clone(&self.mailbox),
        }
    }

    pub fn recv_blocking_with_cancel(&mut self, cancel: &AtomicBool) -> Option<T> {
        let mut state = match self.mailbox.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        loop {
            if cancel.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(value) =
                receive_from_state(state.start_sequence, &state.items, &mut self.next_sequence)
            {
                return Some(value);
            }
            state = match self.mailbox.ready.wait(state) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }
}

#[derive(Debug, Clone)]
pub struct WasmSender<T> {
    state: Rc<RefCell<MailboxState<T>>>,
}

#[derive(Debug)]
pub struct WasmReceiver<T> {
    state: Rc<RefCell<MailboxState<T>>>,
    next_sequence: u64,
}

impl<T: Clone> PlatformSender<T> for WasmSender<T> {
    type Error = SendError;

    fn send(&self, value: T) -> Result<(), Self::Error> {
        self.state.borrow_mut().push(value);
        Ok(())
    }
}

impl<T: Clone> PlatformReceiver<T> for WasmReceiver<T> {
    fn try_recv(&mut self) -> Option<T> {
        let state = self.state.borrow();
        receive_from_state(state.start_sequence, &state.items, &mut self.next_sequence)
    }
}

fn receive_from_state<T: Clone>(
    start_sequence: u64,
    items: &VecDeque<(u64, T)>,
    next_sequence: &mut u64,
) -> Option<T> {
    if *next_sequence < start_sequence {
        *next_sequence = start_sequence;
    }

    let index = (*next_sequence - start_sequence) as usize;
    let value = items.get(index).map(|(_, value)| value.clone())?;
    *next_sequence = next_sequence.saturating_add(1);
    Some(value)
}

impl Platform for NativePlatform {
    type Shared<T: 'static> = Arc<T>;
    type RwLock<T: 'static> = RwLock<T>;
    type ReadGuard<'a, T: 'static>
        = RwLockReadGuard<'a, T>
    where
        Self: 'a,
        T: 'a;
    type WriteGuard<'a, T: 'static>
        = RwLockWriteGuard<'a, T>
    where
        Self: 'a,
        T: 'a;
    type Sender<T: Clone + 'static> = NativeSender<T>;
    type Receiver<T: Clone + 'static> = NativeReceiver<T>;

    fn shared<T: 'static>(value: T) -> Self::Shared<T> {
        Arc::new(value)
    }

    fn new_rwlock<T: 'static>(value: T) -> Self::RwLock<T> {
        RwLock::new(value)
    }

    fn read<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::ReadGuard<'a, T> {
        match lock.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::WriteGuard<'a, T> {
        match lock.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn channel<T: Clone + 'static>(capacity: usize) -> (Self::Sender<T>, Self::Receiver<T>) {
        let mailbox = Arc::new(NativeMailbox::new(capacity));
        (
            NativeSender {
                mailbox: Arc::clone(&mailbox),
            },
            NativeReceiver {
                mailbox,
                next_sequence: 0,
            },
        )
    }
}

impl Platform for WasmPlatform {
    type Shared<T: 'static> = Rc<T>;
    type RwLock<T: 'static> = RefCell<T>;
    type ReadGuard<'a, T: 'static>
        = Ref<'a, T>
    where
        Self: 'a,
        T: 'a;
    type WriteGuard<'a, T: 'static>
        = RefMut<'a, T>
    where
        Self: 'a,
        T: 'a;
    type Sender<T: Clone + 'static> = WasmSender<T>;
    type Receiver<T: Clone + 'static> = WasmReceiver<T>;

    fn shared<T: 'static>(value: T) -> Self::Shared<T> {
        Rc::new(value)
    }

    fn new_rwlock<T: 'static>(value: T) -> Self::RwLock<T> {
        RefCell::new(value)
    }

    fn read<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::ReadGuard<'a, T> {
        lock.borrow()
    }

    fn write<'a, T: 'static>(lock: &'a Self::RwLock<T>) -> Self::WriteGuard<'a, T> {
        lock.borrow_mut()
    }

    fn channel<T: Clone + 'static>(capacity: usize) -> (Self::Sender<T>, Self::Receiver<T>) {
        let state = Rc::new(RefCell::new(MailboxState::new(capacity)));
        (
            WasmSender {
                state: Rc::clone(&state),
            },
            WasmReceiver {
                state,
                next_sequence: 0,
            },
        )
    }
}

pub type LatLngNativeShared<T> = <NativePlatform as Platform>::Shared<T>;
pub type LatLngWasmShared<T> = <WasmPlatform as Platform>::Shared<T>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::{NativePlatform, Platform, PlatformReceiver, PlatformSender, WasmPlatform};

    #[test]
    fn native_rwlock_roundtrip() {
        let lock = NativePlatform::new_rwlock(7_u64);
        assert_eq!(*NativePlatform::read(&lock), 7);
        *NativePlatform::write(&lock) = 9;
        assert_eq!(*NativePlatform::read(&lock), 9);
    }

    #[test]
    fn native_mailbox_preserves_order() {
        let (sender, mut receiver) = NativePlatform::channel(4);
        sender.send("one").unwrap();
        sender.send("two").unwrap();

        assert_eq!(receiver.try_recv(), Some("one"));
        assert_eq!(receiver.try_recv(), Some("two"));
        assert_eq!(receiver.try_recv(), None);
    }

    #[test]
    fn native_mailbox_drops_oldest_when_full() {
        let (sender, mut receiver) = NativePlatform::channel(2);
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        sender.send(3).unwrap();

        assert_eq!(receiver.try_recv(), Some(2));
        assert_eq!(receiver.try_recv(), Some(3));
        assert_eq!(receiver.try_recv(), None);
    }

    #[test]
    fn native_mailbox_blocking_wait_wakes_on_send() {
        let (sender, mut receiver) = NativePlatform::channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let waiting = Arc::clone(&cancel);

        let handle = thread::spawn(move || receiver.recv_blocking_with_cancel(waiting.as_ref()));
        thread::sleep(Duration::from_millis(20));
        sender.send("event").unwrap();

        assert_eq!(handle.join().unwrap(), Some("event"));
    }

    #[test]
    fn native_mailbox_blocking_wait_can_be_cancelled() {
        let (_sender, mut receiver) = NativePlatform::channel::<u32>(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = receiver.wake_handle();
        let waiting = Arc::clone(&cancel);

        let handle = thread::spawn(move || receiver.recv_blocking_with_cancel(waiting.as_ref()));
        thread::sleep(Duration::from_millis(20));
        cancel.store(true, Ordering::SeqCst);
        wake.wake();

        assert_eq!(handle.join().unwrap(), None);
    }

    #[test]
    fn wasm_rwlock_roundtrip() {
        let lock = WasmPlatform::new_rwlock(String::from("a"));
        assert_eq!(&*WasmPlatform::read(&lock), "a");
        *WasmPlatform::write(&lock) = String::from("b");
        assert_eq!(&*WasmPlatform::read(&lock), "b");
    }
}

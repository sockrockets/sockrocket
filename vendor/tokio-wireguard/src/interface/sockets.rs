use crate::io::AllocationGuard;
use std::sync::Arc;

use parking_lot::{ArcMutexGuard, RawMutex};
use smoltcp::{
    iface::{SocketHandle, SocketSet},
    socket::{tcp, udp, Socket},
};

pub struct Sockets {
    set: SocketSet<'static>,
    tcp: SocketsInner<tcp::Socket<'static>>,
    udp: SocketsInner<udp::Socket<'static>>,
}

impl Sockets {
    pub fn new() -> Self {
        Self {
            set: SocketSet::new(Vec::new()),
            tcp: SocketsInner::new(),
            udp: SocketsInner::new(),
        }
    }

    pub fn register_tcp(
        &mut self,
        socket: crate::Shared<tcp::Socket<'static>>,
        lease: Option<Arc<AllocationGuard>>,
    ) {
        self.tcp.register(socket, lease);
    }

    pub fn register_udp(
        &mut self,
        socket: crate::Shared<udp::Socket<'static>>,
        lease: Option<Arc<AllocationGuard>>,
    ) {
        self.udp.register(socket, lease);
    }

    pub fn close(&mut self) {
        self.tcp.close();
        self.udp.close();
    }

    pub fn with<T>(&mut self, f: impl FnOnce(&mut SocketSet) -> T) -> T {
        let Self { set, tcp, udp } = self;

        tcp.lock(set);
        udp.lock(set);
        let result = f(set);

        tcp.unlock(set);
        udp.unlock(set);
        tcp.release_closed();
        udp.release_closed();
        result
    }
}

struct SocketsInner<T> {
    sockets: Vec<crate::Shared<T>>,
    leases: Vec<Option<Arc<AllocationGuard>>>,
    guards: Vec<(SocketHandle, ArcMutexGuard<RawMutex, Option<T>>)>,
}

impl<T> SocketsInner<T> {
    const fn new() -> Self {
        Self {
            sockets: Vec::new(),
            leases: Vec::new(),
            guards: Vec::new(),
        }
    }

    fn release_closed(&mut self) {
        for (socket, lease) in self.sockets.iter().zip(&mut self.leases) {
            if socket.lock().is_none() {
                lease.take();
            }
        }
    }
    fn register(&mut self, socket: crate::Shared<T>, lease: Option<Arc<AllocationGuard>>) {
        // Only reuse entries with no public I/O owner. Replacing an Arc
        // still held by a live socket silently disconnects an unrelated flow.
        let slot = self
            .sockets
            .iter()
            .position(|socket| Arc::strong_count(socket) == 1 && socket.lock().is_none());

        // At most 128 admitted live flows in the host adapter. Keep a bounded
        // allowance for draining closed TCP streams; reclaim an unowned entry
        // under sustained churn instead of retaining TIME_WAIT buffers forever.
        let slot = slot.or_else(|| {
            (self.sockets.len() >= 256)
                .then(|| self.sockets.iter().position(|s| Arc::strong_count(s) == 1))
                .flatten()
        });
        if let Some(index) = slot {
            self.sockets[index] = socket;
            self.leases[index] = lease;
        } else {
            self.sockets.push(socket);
            self.leases.push(lease);
        }
    }
}

impl SocketsInner<tcp::Socket<'static>> {
    fn close(&mut self) {
        for socket in &mut self.sockets {
            if let Some(socket) = &mut *socket.lock() {
                socket.abort();
            }
        }
    }

    fn lock(&mut self, set: &mut SocketSet<'static>) {
        let Self {
            sockets, guards, ..
        } = self;

        for socket in sockets {
            let mut guard = socket.lock_arc();
            if let Some(mut socket) = guard.take() {
                if Arc::strong_count(ArcMutexGuard::mutex(&guard)) == 2 {
                    socket.close();
                }
                guards.push((set.add(socket), guard));
            }
        }
    }

    fn unlock(&mut self, set: &mut SocketSet<'static>) {
        let Self { guards, .. } = self;

        while let Some((handle, mut guard)) = guards.pop() {
            if let Socket::Tcp(socket) = set.remove(handle) {
                if socket.state() == tcp::State::Closed
                    && Arc::strong_count(ArcMutexGuard::mutex(&guard)) == 2
                {
                    continue;
                }
                guard.replace(socket);
            }
        }
    }
}

impl SocketsInner<udp::Socket<'static>> {
    fn close(&mut self) {
        for socket in &mut self.sockets {
            if let Some(socket) = &mut *socket.lock() {
                socket.close();
            }
        }
    }

    fn lock(&mut self, set: &mut SocketSet<'static>) {
        let Self {
            sockets, guards, ..
        } = self;

        for socket in sockets {
            let mut guard = socket.lock_arc();
            if let Some(socket) = guard.take() {
                guards.push((set.add(socket), guard));
            }
        }
    }

    fn unlock(&mut self, set: &mut SocketSet<'static>) {
        let Self { guards, .. } = self;

        while let Some((handle, mut guard)) = guards.pop() {
            if let Socket::Udp(socket) = set.remove(handle) {
                if Arc::strong_count(ArcMutexGuard::mutex(&guard)) == 2 {
                    continue;
                }
                guard.replace(socket);
            }
        }
    }
}

use libmdns::{Responder, Service};
use log::{debug, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::pointer;

/// An mDNS Responder. Used to announce the Accessory's name and HAP TXT records to potential controllers.
pub struct MdnsResponder {
    config: pointer::Config,
    responder: Option<Responder>,
    service: Option<Service>,
    task: Option<Box<dyn futures::Future<Output = ()> + Unpin + std::marker::Send>>,
}

impl MdnsResponder {
    /// Creates a new mDNS Responder.
    pub async fn new(config: pointer::Config) -> Self {
        let (responder, task) =
            libmdns::Responder::with_default_handle().expect("creating mDNS responder");

        MdnsResponder {
            config,
            responder: Some(responder),
            service: None,
            task: Some(task),
        }
    }

    /// Derives new mDNS TXT records from the server's `Config`.
    pub async fn update_records(&mut self) {
        debug!("attempting to set mDNS records");

        self.unregister_current_service();

        let c = self.config.lock().await;

        let name = c.name.clone();
        let port = c.port;
        let tr = c.txt_records();

        drop(c);

        let Some(responder) = self.responder.as_ref() else {
            warn!("mDNS responder is unavailable; cannot set records");
            return;
        };

        let service = catch_unwind(AssertUnwindSafe(|| {
            responder.register(
                "_hap._tcp".into(),
                &name,
                port,
                &[
                    &tr[0], &tr[1], &tr[2], &tr[3], &tr[4], &tr[5], &tr[6], &tr[7],
                ],
            )
        }));

        match service {
            Ok(service) => {
                self.service = Some(service);
            }
            Err(_) => {
                warn!("mDNS responder panicked while setting records; suppressing cleanup panic");
                self.forget_libmdns_handles();
            }
        }

        debug!("setting mDNS records: {:?}", &tr);
    }

    /// Returns the mDNS task to throw on a scheduler.
    pub fn run_handle(
        &mut self,
    ) -> Box<dyn futures::Future<Output = ()> + Unpin + std::marker::Send> {
        match self.task.take() {
            Some(task) => task,
            // if the task handle is gone, recreate the whole responder
            None => {
                self.forget_libmdns_handles();
                let (responder, task) =
                    libmdns::Responder::with_default_handle().expect("creating mDNS responder");
                self.responder = Some(responder);

                task
            }
        }
    }

    fn unregister_current_service(&mut self) {
        let Some(service) = self.service.take() else {
            return;
        };

        if catch_unwind(AssertUnwindSafe(|| drop(service))).is_err() {
            warn!("mDNS service unregister panicked; suppressing cleanup panic");
            self.forget_libmdns_handles();
        }
    }

    fn forget_libmdns_handles(&mut self) {
        if let Some(service) = self.service.take() {
            std::mem::forget(service);
        }
        if let Some(responder) = self.responder.take() {
            std::mem::forget(responder);
        }
        if let Some(task) = self.task.take() {
            std::mem::forget(task);
        }
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        // libmdns 0.10 panics if its Service/Responder destructors send
        // shutdown commands after the responder future has already been
        // cancelled. HAP server shutdown often happens in that order when a
        // Tokio runtime is exiting, so intentionally leak the tiny libmdns
        // handles during process teardown instead of aborting the process.
        self.forget_libmdns_handles();
    }
}

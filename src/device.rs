use std::sync::Arc;

use anyhow::Result;
use asr::devices::{
    start_device_controller, DeviceControllerHandle, DeviceEvent as ToonDeviceEvent,
    DeviceEventSink as ToonDeviceEventSink, DeviceStateSnapshot,
};

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    StateChanged(DeviceStateSnapshot),
    Error(String),
}

pub struct DeviceController {
    handle: Arc<dyn DeviceControllerHandle>,
}

impl DeviceController {
    pub fn start(
        preferred_id: Option<String>,
        event_handler: Arc<dyn Fn(DeviceEvent) + Send + Sync>,
    ) -> Result<Self> {
        let sink: Arc<dyn ToonDeviceEventSink> = Arc::new(ForwardingSink {
            handler: event_handler,
        });
        let handle = start_device_controller(preferred_id, sink)?;
        Ok(Self { handle })
    }

    pub fn snapshot(&self) -> Result<DeviceStateSnapshot> {
        self.handle.snapshot()
    }

    pub fn set_preferred(&self, preferred_id: Option<String>) -> Result<()> {
        self.handle.set_preferred(preferred_id)
    }

    pub fn refresh_now(&self) -> Result<()> {
        self.handle.refresh_now()
    }
}

struct ForwardingSink {
    handler: Arc<dyn Fn(DeviceEvent) + Send + Sync>,
}

impl ToonDeviceEventSink for ForwardingSink {
    fn on_event(&self, event: ToonDeviceEvent) {
        let mapped = match event {
            ToonDeviceEvent::StateChanged(snapshot) => DeviceEvent::StateChanged(snapshot),
            ToonDeviceEvent::Error(message) => DeviceEvent::Error(message),
        };
        (self.handler)(mapped);
    }
}

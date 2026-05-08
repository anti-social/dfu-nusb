use std::time::Duration;
use dfu_core::DfuProtocol;
use dfu_core::functional_descriptor::FunctionalDescriptor;
use dfu_core::memory_layout::MemoryLayout;
use nusb::{Device, Interface, MaybeFuture};
use nusb::descriptors::ConfigurationDescriptor;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};
use thiserror::Error;

pub type Dfu = dfu_core::synchronous::DfuSync<DfuNusb, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Could not find device or an error occurred.")]
    CouldNotOpenDevice,
    #[error(transparent)]
    Dfu(#[from] dfu_core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Usb error")]
    Usb(#[from] nusb::Error),
    #[error("Usb get descriptor error")]
    UsbGetDescriptor(#[from] nusb::GetDescriptorError),
    #[error(transparent)]
    Transfer(#[from] TransferError),
    #[error("The device has no languages.")]
    MissingLanguage,
    #[error("Could not find interface.")]
    InvalidInterface,
    #[error("Could not find alt interface.")]
    InvalidAlt,
    #[error("Could not parse functional descriptor: {0}")]
    FunctionalDescriptor(#[from] dfu_core::functional_descriptor::Error),
    #[error("No DFU capable device found.")]
    NoDfuCapableDeviceFound,
}

pub struct DfuNusb {
    dev: Device,
    iface: Interface,
    protocol: DfuProtocol<MemoryLayout>,
    timeout: Duration,
    functional_descriptor: FunctionalDescriptor,
}

impl dfu_core::DfuIo for DfuNusb {
    type Read = usize;
    type Write = usize;
    type Reset = ();
    type Error = Error;
    type MemoryLayout = MemoryLayout;

    #[allow(unused_variables)]
    fn read_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        buffer: &mut [u8],
    ) -> Result<Self::Read, Self::Error> {
        let (control_type, recipient) = explode_request_type(request_type);
        let res = self.iface.control_in(
            ControlIn {
                control_type,
                recipient,
                request,
                value,
                index: self.iface.interface_number() as u16,
                length: buffer.len() as u16,
            },
            self.timeout,
        ).wait()?;
        buffer[..res.len()].copy_from_slice(&res);
        Ok(res.len())
    }

    #[allow(unused_variables)]
    fn write_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        buffer: &[u8],
    ) -> Result<Self::Write, Self::Error> {
        let (control_type, recipient) = explode_request_type(request_type);
        let res = self.iface.control_out(
            ControlOut {
                control_type,
                recipient,
                request,
                value,
                index: self.iface.interface_number() as u16,
                data: buffer,
            },
            self.timeout,
        ).wait()?;
        Ok(buffer.len())
    }

    fn usb_reset(self) -> Result<Self::Reset, Self::Error> {
        Ok(self.dev.reset().wait()?)
    }

    fn protocol(&self) -> &DfuProtocol<Self::MemoryLayout> {
        &self.protocol
    }

    fn functional_descriptor(&self) -> &FunctionalDescriptor {
        &self.functional_descriptor
    }
}

impl DfuNusb {
    pub fn open(vid: u16, pid: u16, iface: u8, alt: u8) -> Result<Dfu, Error> {
        let device = Self::open_device(vid, pid)?;
        Self::from_usb_device(device, iface, alt)
    }

    fn open_device(
        vid: u16,
        pid: u16,
    ) -> Result<Device, Error> {
        nusb::list_devices().wait()?
            .find(|dev_info| dev_info.vendor_id() == vid && dev_info.product_id() == pid)
            .ok_or(Error::CouldNotOpenDevice)
            .and_then(|dev_info| dev_info.open().wait().map_err(|e| e.into()))
    }

    pub fn from_usb_device(
        device: Device,
        iface_num: u8,
        alt: u8,
    ) -> Result<Dfu, Error> {
        let timeout = std::time::Duration::from_secs(3);
        let iface = device.claim_interface(iface_num).wait()?;
        iface.set_alt_setting(alt).wait()?;
        for config in device.configurations() {
            if let Some(func_desc) = Self::find_functional_descriptor(&device, &config, timeout)
                .transpose()? {
                    let interface = config.interfaces()
                        .find(|x| x.interface_number() == iface_num)
                        .ok_or(Error::InvalidInterface)?;
                    let setting = interface.alt_settings()
                        .find(|x| x.alternate_setting() == alt)
                        .ok_or(Error::InvalidAlt)?;
                    if let Some(string_ix) = setting.string_index() {
                        let iface_string = device.get_string_descriptor(
                            string_ix, 0, Duration::from_millis(1000)
                        )
                            .wait()?
                            .trim_end_matches('\0').to_string();
                        let protocol = dfu_core::DfuProtocol::new(
                            &iface_string,
                            func_desc.dfu_version,
                        )?;

                        let io = DfuNusb {
                            dev: device.clone(),
                            iface: iface,
                            protocol,
                            timeout,
                            functional_descriptor: func_desc,
                        };

                        return Ok(dfu_core::synchronous::DfuSync::new(io));
                    }
                }
        }

        Err(Error::NoDfuCapableDeviceFound)
    }

    pub fn find_functional_descriptor(
        _device: &Device,
        config: &ConfigurationDescriptor,
        _timeout: Duration,
    ) -> Option<Result<FunctionalDescriptor, Error>> {
        for desc_data in config.descriptors().as_bytes().chunks(9) {
            if let Some(func_desc) = FunctionalDescriptor::from_bytes(desc_data) {
                return Some(func_desc.map_err(Into::into));
            }
        }

        None
    }
}

fn explode_request_type(request_type: u8) -> (ControlType, Recipient) {
    let control_type = match (request_type >> 5) & 0b11 {
        0 => ControlType::Standard,
        1 => ControlType::Class,
        _ => ControlType::Vendor,
    };
    let recipient = match request_type & 0b11 {
        0 => Recipient::Device,
        1 => Recipient::Interface,
        2 => Recipient::Endpoint,
        _ => Recipient::Other,
    };
    (control_type, recipient)
}

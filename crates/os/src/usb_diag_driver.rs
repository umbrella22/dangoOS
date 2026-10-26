//! Diagnostic adapter: record the actual CDC bulk IN allocation, delegate unchanged.
use embassy_usb::driver::{
    Driver as UsbDriver, Endpoint, EndpointAddress, EndpointAllocError, EndpointType,
};
use esp_hal::usb::otg::embassy_usb_device::Driver as Inner;

pub struct Driver<'d>(pub Inner<'d>);
impl<'d> UsbDriver<'d> for Driver<'d> {
    type EndpointOut = <Inner<'d> as UsbDriver<'d>>::EndpointOut;
    type EndpointIn = <Inner<'d> as UsbDriver<'d>>::EndpointIn;
    type ControlPipe = <Inner<'d> as UsbDriver<'d>>::ControlPipe;
    type Bus = <Inner<'d> as UsbDriver<'d>>::Bus;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointOut, EndpointAllocError> {
        self.0
            .alloc_endpoint_out(ep_type, ep_addr, max_packet_size, interval_ms)
    }
    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointIn, EndpointAllocError> {
        let ep = self
            .0
            .alloc_endpoint_in(ep_type, ep_addr, max_packet_size, interval_ms)?;
        if ep_type == EndpointType::Bulk {
            os::telemetry::usb_session::allocated_bulk_in(ep.info().addr.index());
        }
        Ok(ep)
    }
    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        self.0.start(control_max_packet_size)
    }
}

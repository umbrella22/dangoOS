//! Persistent mutations stop below Dhara and LittleFS, including sync and Drop.
use fs::{
    nand::{self, Nand},
    BlockDevice, Errno,
};

pub struct ReadOnlyNand<N>(pub N);
impl<N: Nand> Nand for ReadOnlyNand<N> {
    fn blocks(&self) -> u32 {
        self.0.blocks()
    }
    fn is_bad(&mut self, block: u32) -> Result<bool, nand::Error> {
        self.0.is_bad(block)
    }
    fn is_free(&mut self, page: u32) -> Result<bool, nand::Error> {
        self.0.is_free(page)
    }
    fn read(&mut self, page: u32, offset: usize, bytes: &mut [u8]) -> Result<(), nand::Error> {
        self.0.read(page, offset, bytes)
    }
    fn mark_bad(&mut self, _: u32) -> Result<(), nand::Error> {
        Err(nand::Error::Io)
    }
    fn erase(&mut self, _: u32) -> Result<(), nand::Error> {
        Err(nand::Error::Io)
    }
    fn program(&mut self, _: u32, _: &[u8]) -> Result<(), nand::Error> {
        Err(nand::Error::Io)
    }
}
pub struct ReadOnlyBlock<B>(pub B);
impl<B: BlockDevice> BlockDevice for ReadOnlyBlock<B> {
    fn read(&mut self, block: u32, offset: u32, bytes: &mut [u8]) -> Result<(), Errno> {
        self.0.read(block, offset, bytes)
    }
    fn prog(&mut self, _: u32, _: u32, _: &[u8]) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
    fn erase(&mut self, _: u32) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
    // A read-only view has no dirty mapping or pending data to persist.
    fn sync(&mut self) -> Result<(), Errno> {
        Ok(())
    }
}

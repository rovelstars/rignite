use uefi::proto::console::text::{Input, Key, ScanCode};

pub struct InputHandler<'a> {
    input: &'a mut Input,
}

impl<'a> InputHandler<'a> {
    pub fn new(input: &'a mut Input) -> Self {
        Self { input }
    }

    pub fn read_key(&mut self) -> Option<Key> {
        self.input.read_key().ok().flatten()
    }
}

use std::sync::Mutex;

#[derive(Debug)]
pub struct RingBuffer {
    buffer: Mutex<Vec<u8>>,
    size: usize,
    write_offset: Mutex<usize>,
    read_offset: Mutex<usize>,
    length: Mutex<usize>,
}

impl RingBuffer {
    pub fn new(size: usize) -> Self {
        Self {
            buffer: Mutex::new(vec![0u8; size]),
            size,
            write_offset: Mutex::new(0),
            read_offset: Mutex::new(0),
            length: Mutex::new(0),
        }
    }

    pub fn len(&self) -> usize {
        *self.length.lock().unwrap()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.size
    }

    pub fn write(&self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }

        let mut buffer = self.buffer.lock().unwrap();
        let mut write_offset = self.write_offset.lock().unwrap();
        let mut read_offset = self.read_offset.lock().unwrap();
        let mut length = self.length.lock().unwrap();

        let bytes_to_write = chunk.len().min(self.size);
        let available_at_end = self.size - *write_offset;

        if bytes_to_write <= available_at_end {
            buffer[*write_offset..*write_offset + bytes_to_write].copy_from_slice(&chunk[..bytes_to_write]);
        } else {
            buffer[*write_offset..].copy_from_slice(&chunk[..available_at_end]);
            buffer[..bytes_to_write - available_at_end]
                .copy_from_slice(&chunk[available_at_end..bytes_to_write]);
        }

        let new_length = *length + bytes_to_write;
        if new_length > self.size {
            *read_offset = (*read_offset + (new_length - self.size)) % self.size;
            *length = self.size;
        } else {
            *length = new_length;
        }
        *write_offset = (*write_offset + bytes_to_write) % self.size;
    }

    pub fn read(&self, n: usize) -> Option<Vec<u8>> {
        let buffer = self.buffer.lock().unwrap();
        let mut read_offset = self.read_offset.lock().unwrap();
        let mut length = self.length.lock().unwrap();

        let bytes_to_read = n.min(*length);
        if bytes_to_read == 0 {
            return None;
        }

        let mut out = vec![0u8; bytes_to_read];
        let available_at_end = self.size - *read_offset;

        if bytes_to_read <= available_at_end {
            out.copy_from_slice(&buffer[*read_offset..*read_offset + bytes_to_read]);
        } else {
            out[..available_at_end].copy_from_slice(&buffer[*read_offset..]);
            out[available_at_end..].copy_from_slice(&buffer[..bytes_to_read - available_at_end]);
        }

        *read_offset = (*read_offset + bytes_to_read) % self.size;
        *length -= bytes_to_read;

        Some(out)
    }

    pub fn skip(&self, n: usize) -> usize {
        let mut read_offset = self.read_offset.lock().unwrap();
        let mut length = self.length.lock().unwrap();

        let bytes_to_skip = n.min(*length);
        *read_offset = (*read_offset + bytes_to_skip) % self.size;
        *length -= bytes_to_skip;
        bytes_to_skip
    }

    pub fn peek(&self, n: usize) -> Option<Vec<u8>> {
        let buffer = self.buffer.lock().unwrap();
        let read_offset = *self.read_offset.lock().unwrap();
        let length = *self.length.lock().unwrap();

        let bytes_to_peek = n.min(length);
        if bytes_to_peek == 0 {
            return None;
        }

        let available_at_end = self.size - read_offset;
        if bytes_to_peek <= available_at_end {
            Some(buffer[read_offset..read_offset + bytes_to_peek].to_vec())
        } else {
            let mut out = vec![0u8; bytes_to_peek];
            out[..available_at_end].copy_from_slice(&buffer[read_offset..]);
            out[available_at_end..].copy_from_slice(&buffer[..bytes_to_peek - available_at_end]);
            Some(out)
        }
    }

    pub fn get_contiguous(&self, n: usize) -> Option<Vec<u8>> {
        let buffer = self.buffer.lock().unwrap();
        let read_offset = *self.read_offset.lock().unwrap();
        let length = *self.length.lock().unwrap();

        let bytes_to_get = n.min(length);
        if bytes_to_get == 0 {
            return None;
        }

        let available_at_end = self.size - read_offset;
        if bytes_to_get <= available_at_end {
            Some(buffer[read_offset..read_offset + bytes_to_get].to_vec())
        } else {
            let mut out = vec![0u8; bytes_to_get];
            out[..available_at_end].copy_from_slice(&buffer[read_offset..]);
            out[available_at_end..].copy_from_slice(&buffer[..bytes_to_get - available_at_end]);
            Some(out)
        }
    }

    pub fn clear(&self) {
        *self.write_offset.lock().unwrap() = 0;
        *self.read_offset.lock().unwrap() = 0;
        *self.length.lock().unwrap() = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_write_read() {
        let rb = RingBuffer::new(1024);
        rb.write(b"hello");
        assert_eq!(rb.len(), 5);
        let data = rb.read(5).unwrap();
        assert_eq!(&data, b"hello");
        assert_eq!(rb.len(), 0);
    }

    #[test]
    fn test_wrap_around() {
        let rb = RingBuffer::new(10);
        rb.write(b"12345678");
        assert_eq!(rb.len(), 8);
        rb.read(5);
        assert_eq!(rb.len(), 3);
        rb.write(b"abcde");
        assert_eq!(rb.len(), 8);
        let data = rb.read(8).unwrap();
        assert_eq!(&data, b"678abcde");
    }

    #[test]
    fn test_overwrite_on_full() {
        let rb = RingBuffer::new(10);
        rb.write(b"1234567890");
        assert_eq!(rb.len(), 10);
        rb.write(b"abcde");
        assert_eq!(rb.len(), 10);
        let data = rb.read(10).unwrap();
        assert_eq!(&data, b"67890abcde");
    }

    #[test]
    fn test_skip() {
        let rb = RingBuffer::new(1024);
        rb.write(b"hello world");
        assert_eq!(rb.skip(6), 6);
        assert_eq!(rb.len(), 5);
        let data = rb.read(5).unwrap();
        assert_eq!(&data, b"world");
    }

    #[test]
    fn test_peek() {
        let rb = RingBuffer::new(1024);
        rb.write(b"hello");
        let data = rb.peek(3).unwrap();
        assert_eq!(&data, b"hel");
        assert_eq!(rb.len(), 5);
        let data = rb.read(5).unwrap();
        assert_eq!(&data, b"hello");
    }

    #[test]
    fn test_get_contiguous() {
        let rb = RingBuffer::new(10);
        rb.write(b"12345678");
        rb.read(5);
        rb.write(b"abcde");
        let data = rb.get_contiguous(8).unwrap();
        assert_eq!(&data, b"678abcde");
    }
}
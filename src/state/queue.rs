//! Queues are used to store hash, proof verification and payment requests

use borsh::{BorshSerialize, BorshDeserialize};
use solana_program::program_error::ProgramError;
use crate::error::ElusivError::{QueueIsFull, QueueIsEmpty};
use crate::macros::guard;
use crate::bytes::*;
use crate::macros::*;
use crate::processor::{BaseCommitmentHashRequest, CommitmentHashRequest};
use super::program_account::{SizedAccount, ProgramAccount};

/// Generates a `QueueAccount` and a `Queue` that implements the `RingQueue` trait
macro_rules! queue_account {
    ($name: ident, $account: ident, $seed: literal, $size: literal, $ty: ty) => {
        #[elusiv_account(pda_seed = $seed)]
        pub struct $account {
            bump_seed: u8,
            initialized: bool,

            head: u64,
            tail: u64,
            data: [$ty; $size],
        }

        pub struct $name<'a, 'b> {
            account: &'b mut $account<'a>,
        }

        impl<'a, 'b> Queue<'a, 'b, $account<'a>> for $name<'a, 'b> {
            type T = $name<'a, 'b>;
            fn new(account: &'b mut $account<'a>) -> Self::T { $name { account } }
        }
        
        impl<'a, 'b> RingQueue for $name<'a, 'b> {
            type N = $ty;
            const SIZE: u64 = $size * Self::N::SIZE as u64;
        
            fn get_head(&self) -> u64 { self.account.get_head() }
            fn set_head(&mut self, value: &u64) { self.account.set_head(value) }
            fn get_tail(&self) -> u64 { self.account.get_tail() }
            fn set_tail(&mut self, value: &u64) { self.account.set_tail(value) }
            fn get_data(&self, index: usize) -> Self::N { self.account.get_data(index) }
            fn set_data(&mut self, index: usize, value: &Self::N) { self.account.set_data(index, value) }
        }
    };
}

pub trait Queue<'a, 'b, Account: ProgramAccount<'a>> {
    type T;
    fn new(account: &'b mut Account) -> Self::T;
}

// Base commitment queue
queue_account!(BaseCommitmentQueue, BaseCommitmentQueueAccount, b"base_commitment_queue", 128, BaseCommitmentHashRequest);

// Queue used for storing commitments that should sequentially inserted into the active Merkle tree
queue_account!(CommitmentQueue, CommitmentQueueAccount, b"commitment_queue", 240, CommitmentHashRequest);

/// Ring queue with a capacity of `SIZE - 1` elements
/// - works by having two pointers, `head` and `tail` and a some data storage with getter, setter
/// - `head` points to the first element (first according to the FIFO definition)
/// - `tail` points to the location to insert the next element
/// - `head == tail - 1` => queue is full
/// - `head == tail` => queue is empty
pub trait RingQueue {
    type N: PartialEq + BorshSerDeSized + Clone;
    const SIZE: u64;

    fn get_head(&self) -> u64;
    fn set_head(&mut self, value: &u64);

    fn get_tail(&self) -> u64;
    fn set_tail(&mut self, value: &u64);

    fn get_data(&self, index: usize) -> Self::N;
    fn set_data(&mut self, index: usize, value: &Self::N);

    /// Try to enqueue a new element in the queue
    fn enqueue(&mut self, value: Self::N) -> Result<(), ProgramError> {
        let head = self.get_head();
        let tail = self.get_tail();

        let next_tail = (tail + 1) % Self::SIZE;
        guard!(next_tail != head, QueueIsFull);

        self.set_data(tail as usize, &value);
        self.set_tail(&next_tail);

        Ok(())
    }

    /// Try to read the first element in the queue without removing it
    fn view_first(&self) -> Result<Self::N, ProgramError> {
        self.view(0)
    }

    fn view(&self, offset: usize) -> Result<Self::N, ProgramError> {
        let head = self.get_head();
        let tail = self.get_tail();
        guard!(head != tail, QueueIsEmpty);

        Ok(self.get_data((head as usize + offset) % (Self::SIZE as usize)))
    }

    /// Try to remove the first element from the queue
    fn dequeue_first(&mut self) -> Result<Self::N, ProgramError> {
        let head = self.get_head();
        let tail = self.get_tail();

        guard!(head != tail, QueueIsEmpty);

        let value = self.get_data(head as usize);
        self.set_head(&((head + 1) % Self::SIZE));

        Ok(value)
    }

    fn contains(&self, value: &Self::N) -> bool {
        let mut ptr = self.get_head();
        let tail = self.get_tail();

        while ptr != tail {
            if self.get_data(ptr as usize) == *value { return true; }
            ptr = (ptr + 1) % Self::SIZE;
        }

        false
    }

    fn len(&self) -> u64 {
        let head = self.get_head();
        let tail = self.get_tail();

        if tail < head {
            head + tail
        } else {
            tail - head
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: usize = 7;

    struct TestQueue {
        head: u64,
        tail: u64,
        data: [u32; SIZE],
    }

    impl RingQueue for TestQueue {
        type N = u32;
        const SIZE: u64 = SIZE as u64;

        fn get_head(&self) -> u64 { self.head }
        fn set_head(&mut self, value: &u64) { self.head = *value; }

        fn get_tail(&self) -> u64 { self.tail }
        fn set_tail(&mut self, value: &u64) { self.tail = *value; }

        fn get_data(&self, index: usize) -> u32 { self.data[index] }
        fn set_data(&mut self, index: usize, value: &u32) { self.data[index] = *value; }
    }

    macro_rules! test_queue {
        ($id: ident, $head: literal, $tail: literal) => {
            let mut $id = TestQueue { head: $head, tail: $tail, data: [0; SIZE] };
        };
    }

    #[test]
    fn test_persistent_fifo() {
        test_queue!(queue, 0, 0);

        for i in 1..SIZE {
            queue.enqueue(i as u32).unwrap();
            assert_eq!(1, queue.view_first().unwrap()); // first element does not change
            assert_eq!(queue.len(), i as u64);
        }
    }

    #[test]
    fn test_max_size() {
        test_queue!(full_queue, 1, 0);
        assert!(matches!(full_queue.enqueue(1), Err(_)));
    }

    #[test]
    fn test_ordering() {
        test_queue!(queue, 0, 0);

        for i in 1..SIZE {
            queue.enqueue(i as u32).unwrap();
        }

        for i in 1..SIZE {
            assert_eq!(i as u32, queue.view_first().unwrap());
            queue.dequeue_first().unwrap();
        }
        assert!(matches!(queue.dequeue_first(), Err(_)));
    }
}
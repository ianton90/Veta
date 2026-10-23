//! Parsing CSV text on all cores: the text is cut into chunks at record
//! boundaries, the chunks are parsed at the same time, and their rows come
//! back in file order.

use crate::error::invalid;
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};

use arrow::csv::reader::Format;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use crate::error::Result;

/// Bytes of text per chunk.
const CHUNK_BYTES: usize = 16 * 1024 * 1024;

/// Longest record (#160): with an unmatched quote no record ever ends, and
/// the whole file would be read into one buffer.
const MAX_RECORD_BYTES: usize = 256 * 1024 * 1024;

type Parsed = Result<Vec<RecordBatch>>;

/// The rows of CSV text read from `input`, parsed as `schema` (all text)
/// on several threads, in batches of at most `batch_rows`, in order.
pub(super) struct ParallelCsv<'a, R: Read> {
    input: &'a mut R,
    quote: Option<u8>,
    chunk_bytes: usize,
    max_record: usize,
    /// Whether the first record (the header) is still to be skipped.
    header: bool,
    /// Text read but not yet sent: the start of an unfinished record.
    rest: Vec<u8>,
    at_end: bool,
    jobs: Option<SyncSender<(usize, Vec<u8>)>>,
    results: Receiver<(usize, Parsed)>,
    /// Chunks sent so far, and the next one whose rows are returned.
    sent: usize,
    next: usize,
    /// Parsed chunks that came back before an earlier one.
    early: BTreeMap<usize, Parsed>,
    ready: VecDeque<RecordBatch>,
    in_flight: usize,
}

impl<'a, R: Read> ParallelCsv<'a, R> {
    pub(super) fn new(
        input: &'a mut R,
        schema: SchemaRef,
        format: Format,
        header: bool,
        quote: Option<u8>,
        batch_rows: usize,
    ) -> Self {
        let (jobs, queue) = sync_channel::<(usize, Vec<u8>)>(2 * crate::parallel::threads());
        let queue = Arc::new(Mutex::new(queue));
        let (done, results) = channel();
        let format = format.with_header(false);
        for _ in 0..crate::parallel::threads() {
            let queue = queue.clone();
            let done = done.clone();
            let schema = schema.clone();
            let format = format.clone();
            std::thread::spawn(move || {
                loop {
                    let job = queue
                        .lock()
                        .map_err(|_| ())
                        .and_then(|q| q.recv().map_err(|_| ()));
                    let Ok((index, text)) = job else {
                        break;
                    };
                    if done
                        .send((index, parse(&text, &schema, &format, batch_rows)))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        Self {
            input,
            quote,
            chunk_bytes: CHUNK_BYTES,
            max_record: MAX_RECORD_BYTES,
            header,
            rest: Vec::new(),
            at_end: false,
            jobs: Some(jobs),
            results,
            sent: 0,
            next: 0,
            early: BTreeMap::new(),
            ready: VecDeque::new(),
            in_flight: 0,
        }
    }

    #[cfg(test)]
    fn with_chunk_bytes(mut self, bytes: usize) -> Self {
        self.chunk_bytes = bytes;
        self
    }

    #[cfg(test)]
    fn with_max_record(mut self, bytes: usize) -> Self {
        self.max_record = bytes;
        self
    }

    /// Refuses a record still unfinished after `max_record` bytes.
    fn check_record(&self) -> Result<()> {
        if self.rest.len() > self.max_record {
            return Err(invalid!(
                CsvRecordTooLong,
                mb = self.max_record / (1024 * 1024)
            ));
        }
        Ok(())
    }

    /// Reads and sends the next chunk. Returns `false` at the end of the
    /// text.
    fn send_chunk(&mut self) -> Result<bool> {
        let mut target = self.chunk_bytes;
        loop {
            let mut filled = self.rest.len();
            if !self.at_end && filled < target {
                self.rest.resize(target, 0);
                while !self.at_end && filled < target {
                    let n = self.input.read(&mut self.rest[filled..])?;
                    filled += n;
                    self.at_end = n == 0;
                }
                self.rest.truncate(filled);
            }
            if self.header {
                match first_boundary(&self.rest, self.quote) {
                    Some(end) => {
                        self.rest.drain(..=end);
                        self.header = false;
                    }
                    None if self.at_end => {
                        self.rest.clear();
                        self.header = false;
                    }
                    None => {
                        self.check_record()?;
                        target *= 2;
                        continue;
                    }
                }
            }
            let end = if self.at_end {
                self.rest.len()
            } else {
                match last_boundary(&self.rest, self.quote) {
                    Some(end) => end + 1,
                    // A record longer than a chunk: read more.
                    None => {
                        self.check_record()?;
                        target = self.rest.len() * 2;
                        continue;
                    }
                }
            };
            if end == 0 {
                return Ok(false);
            }
            let rest = self.rest.split_off(end);
            let chunk = std::mem::replace(&mut self.rest, rest);
            let jobs = self.jobs.as_ref().expect("sending until the end");
            jobs.send((self.sent, chunk))
                .map_err(|_| invalid!(ReadFailed))?;
            self.sent += 1;
            self.in_flight += 1;
            return Ok(true);
        }
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(batch) = self.ready.pop_front() {
                return Ok(Some(batch));
            }
            if let Some(parsed) = self.early.remove(&self.next) {
                self.next += 1;
                self.in_flight -= 1;
                self.ready.extend(parsed?);
                continue;
            }
            // Keep every thread busy, with one more chunk queued each.
            let limit = 2 * crate::parallel::threads();
            while self.jobs.is_some() && self.in_flight < limit {
                if !self.send_chunk()? {
                    self.jobs = None;
                }
            }
            if self.in_flight == 0 {
                return Ok(None);
            }
            let (index, parsed) = self.results.recv().map_err(|_| invalid!(ReadFailed))?;
            self.early.insert(index, parsed);
        }
    }
}

impl<R: Read> Iterator for ParallelCsv<'_, R> {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_batch().transpose()
    }
}

fn parse(text: &[u8], schema: &SchemaRef, format: &Format, batch_rows: usize) -> Parsed {
    let reader = arrow::csv::ReaderBuilder::new(schema.clone())
        .with_format(format.clone())
        .with_batch_size(batch_rows)
        .build(text)?;
    Ok(reader.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Position of the line feed ending the first record of `text`, which
/// starts at a record boundary: one outside quotes.
fn first_boundary(text: &[u8], quote: Option<u8>) -> Option<usize> {
    let mut quoted = false;
    text.iter().position(|&b| {
        if Some(b) == quote {
            quoted = !quoted;
        }
        b == b'\n' && !quoted
    })
}

/// Position of the line feed ending the last complete record of `text`.
/// Quotes come in pairs within a record (an escaped quote is two), so a
/// line feed ends a record when an even number of quotes come before it.
fn last_boundary(text: &[u8], quote: Option<u8>) -> Option<usize> {
    let Some(quote) = quote else {
        return text.iter().rposition(|&b| b == b'\n');
    };
    let total = text.iter().filter(|&&b| b == quote).count();
    let mut after = 0;
    for (i, &b) in text.iter().enumerate().rev() {
        if b == quote {
            after += 1;
        } else if b == b'\n' && (total - after) % 2 == 0 {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    #[test]
    fn boundaries_skip_line_feeds_in_quotes() {
        let text = b"a,\"x\ny\"\nb,\"\"\"\n\"\nc,d";
        assert_eq!(first_boundary(text, Some(b'"')), Some(7));
        assert_eq!(last_boundary(text, Some(b'"')), Some(15));
        assert_eq!(last_boundary(b"a\nb\"\nc", None), Some(4));
        assert_eq!(last_boundary(b"no line feed", Some(b'"')), None);
    }

    #[test]
    fn rows_come_back_in_order() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("n", DataType::Utf8, true),
            Field::new("s", DataType::Utf8, true),
        ]));
        let mut text = String::from("n,s\n");
        for i in 0..200_000 {
            if i % 1000 == 0 {
                text.push_str(&format!("{i},\"two\nlines, \"\"quoted\"\"\"\n"));
            } else {
                text.push_str(&format!("{i},plain\n"));
            }
        }
        let format = Format::default().with_header(true);
        let mut input = text.as_bytes();
        let csv = ParallelCsv::new(&mut input, schema, format, true, Some(b'"'), 1000)
            .with_chunk_bytes(10_000);
        let batches: Vec<RecordBatch> = csv.map(Result::unwrap).collect();
        let mut row = 0;
        for batch in &batches {
            let n = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let s = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..n.len() {
                assert_eq!(n.value(i), row.to_string());
                let expected = if row % 1000 == 0 {
                    "two\nlines, \"quoted\""
                } else {
                    "plain"
                };
                assert_eq!(s.value(i), expected);
                row += 1;
            }
        }
        assert_eq!(row, 200_000);
    }

    #[test]
    fn unfinished_records_stop_at_the_limit() {
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
        // An unmatched quote: no record ends after it.
        let mut text = String::from("s\na\n\"b\n");
        text.push_str(&"x\n".repeat(1_000_000));
        let mut input = text.as_bytes();
        let csv = ParallelCsv::new(
            &mut input,
            schema,
            Format::default(),
            true,
            Some(b'"'),
            1000,
        )
        .with_chunk_bytes(1000)
        .with_max_record(2 * 1024 * 1024 / 16);
        let error = csv.filter_map(Result::err).next().expect("an error");
        assert!(
            matches!(&error, crate::error::Error::Invalid(e) if e.code == crate::error::ErrorCode::CsvRecordTooLong),
            "{error}"
        );
    }
}

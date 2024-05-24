#![feature(never_type)]
#![feature(let_chains)]
#![feature(strict_provenance)]
#![feature(strict_overflow_ops)]
#![allow(unused_imports)]

use std::{str, mem, process};
use std::path::PathBuf;
use std::borrow::Cow;
use std::io::SeekFrom;

use anyhow::{Error, Result, Context, bail};
use clap::Parser;
use futures::prelude::*;
use tokio::{fs, try_join};
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use zerocopy::AsBytes;

use unreal::*;
use unreal::types::*;
use unreal::parse::*;

/// Set the container ID of an Unreal Engine 4.27 IoStore container.
#[derive(Debug, Parser)]
struct Args {
    /// Path to the container file
    container: PathBuf,
    /// Container ID override
    ///
    /// Defaults to the container file name, excluding the extension, platform ID, and patch designator. This is the same value Unreal uses
    id: Option<String>,

    /// Rewrite the container even if its ID is already the requested one
    #[clap(long)]
    force: bool,
}

fn byte_pos<'a, T>(all: &'a [u8], of: &'a T) -> usize {
    (of as *const T as *const u8).addr() - all.as_ptr().addr()
}
fn strip_platform(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |(n, _)| n)
}
fn strip_patch(name: &str) -> &str {
    if let Some(name) = name.strip_suffix("_P") {
        name.rsplit_once('_').map_or(name, |(n, _)| n)
    } else {
        name
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut container_path = args.container;
    container_path.set_extension("");
    let ref container_path = container_path;
    let new_container_id = args.id
        .map_or_else(
            || {
                let name = container_path.file_name()
                    .context("is a directory")?
                    .to_str()
                    .context("non-unicode filename")?;
                // we already removed the extension, so just strip name stuff
                anyhow::Ok(FIoContainerId::from_name(strip_platform(strip_patch(name))))
            },
            |n| Ok(FIoContainerId::from_name(&n)),
        )?;

    let mut toc_file = fs::File::options()
        .read(true)
        .write(true)
        .open(toc_path(container_path)).await
        .context("opening toc")?;

    let mut toc_data = Vec::new();
    toc_file.read_to_end(&mut toc_data).await
        .context("reading toc")?;
    let toc = Archive::read::<Toc>(&toc_data)
        .context("parsing toc")?;

    if toc.header.container_id == new_container_id && !args.force {
        eprintln!("container ID is already {:?} (pass --force to rewrite anyway)", new_container_id);
        process::exit(1)
    }
    eprintln!("rewriting {:?} to {:?}...", toc.header.container_id, new_container_id);

    let partition_count = toc.header.partition_count();
    let mut partitions = future::try_join_all((0..partition_count).map(|i| async move {
        fs::File::options()
            .read(true)
            .write(true)
            .open(partition_path(container_path, i)).await
            .context("opening container")
    })).await?;

    if !toc.chunk_perfect_hash_seeds.is_empty() {
        bail!("rebuilding chunk PHT is unimplemented")
    }
    if toc.header.container_flags.signed() {
        bail!("signed containers are unimplemented")
    }
    if toc.header.container_flags.encrypted() {
        bail!("encrypted containers are unimplemented")
    }

    let old_header_chunk = FIoChunkId::container_header(toc.header.container_id);
    let new_header_chunk = FIoChunkId::container_header(new_container_id);
    let header_chunk_index = toc.find_chunk(&old_header_chunk)
        .context("missing container header chunk")?;

    let header_chunk_range = toc.offset_length(header_chunk_index)
        .context("malformed toc")?
        .resolve_range(..)
        .context("container header chunk range is invalid")?;
    let mut header_data = toc.read_async(header_chunk_range.clone(), |i| {
        let partitions = &mut partitions;
        async move { Ok(&mut partitions[i]) }
    }).await
        .context("reading container header")?;
    let header_props = Archive::new(&header_data).next::<&FContainerHeader_Props>()
        .context("parsing container header")?;

    // rewrite container id in toc header
    let toc_header_loc = byte_pos(&toc_data, &toc.header.container_id);
    toc_file.seek(SeekFrom::Start(toc_header_loc.try_into().unwrap())).await?;
    toc_file.write_all(new_container_id.as_bytes()).await?;

    // rewrite container header chunk id in toc chunk list
    let chunk_id_loc = byte_pos(&toc_data, &toc.chunk_ids[header_chunk_index]);
    toc_file.seek(SeekFrom::Start(chunk_id_loc.try_into().unwrap())).await?;
    toc_file.write_all(new_header_chunk.as_bytes()).await?;

    // rewrite container id in container header data
    let header_container_offset = byte_pos(&header_data, &header_props.container_id);
    if header_container_offset != 0 {
        bail!("container id in wrong place")
    }
    header_data[header_container_offset..header_container_offset + mem::size_of::<FIoContainerId>()].copy_from_slice(new_container_id.as_bytes());

    // re-compress the first block of the container header
    let (first_block_index, offset_in_block) = toc.header.compression_block(header_chunk_range.start);
    if offset_in_block != 0 {
        bail!("container header isn't aligned to a block")
    }
    let old_first_block_entry = &toc.compression_blocks[first_block_index];
    let (partition, first_block_offset) = toc.header.partition(old_first_block_entry.offset());
    let container_file = &mut partitions[partition];

    let first_block_data = header_data.chunks(toc.compression_block_size().try_into().unwrap()).next().unwrap();
    let compression_method = toc.compression_method(old_first_block_entry.compression_method_index());
    let mut new_first_block_compressed = if let Some(method) = compression_method {
        Compressor::with(|c| c.compress_to_vec(method, first_block_data))?.into()
    } else {
        Cow::Borrowed(first_block_data)
    };
    let new_first_block_uncompressed_size = u32::try_from(first_block_data.len()).unwrap();
    let new_first_block_compressed_size = u32::try_from(new_first_block_compressed.len()).unwrap();
    let new_first_block_aligned_compressed_size = new_first_block_compressed_size.next_multiple_of(AES_BLOCK_SIZE);

    // shift the trailing compression blocks
    let delta_len = isize::try_from(new_first_block_aligned_compressed_size).unwrap() - isize::try_from(old_first_block_entry.aligned_compressed_size()).unwrap();
    let delta_len_64 = i64::try_from(delta_len).unwrap();
    let len_shift = u64::try_from(delta_len.unsigned_abs()).unwrap();

    const BUFFER_SIZE: usize = 4096;
    const BUFFER_SIZE_64: u64 = BUFFER_SIZE as u64;
    let mut buf = [0u8; BUFFER_SIZE];
    let old_first_block_end = first_block_offset + u64::from(old_first_block_entry.aligned_compressed_size_u32());
    if delta_len > 0 {
        eprintln!("growing initial container header compression block by {len_shift} B");
        // copy forward, starting at end
        let mut pos = container_file.seek(SeekFrom::End(0)).await?;
        while pos > old_first_block_end {
            let new_pos = pos.saturating_sub(BUFFER_SIZE_64).max(old_first_block_end);
            let len = usize::try_from(pos - new_pos).unwrap();
            container_file.seek(SeekFrom::Start(new_pos)).await?;
            container_file.read_exact(&mut buf[..len]).await?;
            container_file.seek(SeekFrom::Start(new_pos + len_shift)).await?;
            container_file.write_all(&buf[..len]).await?;
            pos = new_pos;
        }
    } else if delta_len < 0 {
        eprintln!("shrinking initial container header compression block by {len_shift} B");
        // copy backward, starting at beginning
        let mut pos = old_first_block_end;
        loop {
            container_file.seek(SeekFrom::Start(pos)).await?;
            let n = container_file.read(&mut buf).await?;
            if n == 0 { break }
            container_file.seek(SeekFrom::Start(pos - len_shift)).await?;
            container_file.write_all(&buf[..n]).await?;
            pos += u64::try_from(n).unwrap();
        }
        container_file.set_len(pos).await?;
    }

    // write out the first compression block of the container header
    container_file.seek(SeekFrom::Start(first_block_offset)).await?;
    if let Cow::Owned(bytes) = &mut new_first_block_compressed {
        // pad out to aligned length
        bytes.resize(new_first_block_aligned_compressed_size.try_into().unwrap(), 0);
    }
    container_file.write_all(&new_first_block_compressed).await?;

    // finally, update all the compression block entries in the toc
    let new_block_entries = toc.compression_blocks[first_block_index..].iter()
        .enumerate()
        .map(|(i, old_block_entry)| if i == 0 {
            FIoStoreTocCompressedBlockEntry::from_parts(
                old_block_entry.offset(),
                new_first_block_compressed_size,
                new_first_block_uncompressed_size,
                old_block_entry.compression_method_index(),
            )
        } else {
            FIoStoreTocCompressedBlockEntry::from_parts(
                old_block_entry.offset().strict_add_signed(delta_len_64),
                old_block_entry.compressed_size_u32(),
                old_block_entry.uncompressed_size_u32(),
                old_block_entry.compression_method_index(),
            )
        })
        .collect::<Vec<_>>();
    let first_block_entry_offset = byte_pos(&toc_data, old_first_block_entry);
    toc_file.seek(SeekFrom::Start(first_block_entry_offset.try_into().unwrap())).await?;
    toc_file.write_all(new_block_entries.as_bytes()).await?;

    eprintln!("done");
    Ok(())
}

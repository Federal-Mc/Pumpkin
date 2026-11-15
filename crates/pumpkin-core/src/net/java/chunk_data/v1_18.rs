use super::util::write_compound_nbt;
use pumpkin_protocol::codec::bit_set::BitSet;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_protocol::ser::WritingError;
use pumpkin_util::math::position::get_local_cord;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk::palette::NetworkPalette;
use std::io::Write;

/// Serializes chunk data for Minecraft 1.18+ through 26.2+.
#[expect(clippy::too_many_lines)]
pub fn write_chunk_data(
    chunk: &ChunkData,
    mut write: impl Write,
    version: &JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_i32_be(chunk.x)?;
    write.write_i32_be(chunk.z)?;

    let heightmaps = chunk
        .heightmap
        .lock()
        .map_err(|_| WritingError::Message("heightmap lock poisoned".into()))?;
    if version >= &JavaMinecraftVersion::V_1_21_5 {
        write.write_var_int(&VarInt(3))?; // Map size

        let mut write_heightmap = |index: i32, data: &[i64]| -> Result<(), WritingError> {
            write.write_var_int(&VarInt(index))?;
            write.write_var_int(&VarInt(data.len() as i32))?;
            for val in data {
                write.write_i64_be(*val)?;
            }
            Ok(())
        };

        write_heightmap(1, heightmaps.world_surface.as_deref().unwrap_or(&[0; 37]))?;
        write_heightmap(4, heightmaps.motion_blocking.as_deref().unwrap_or(&[0; 37]))?;
        write_heightmap(
            5,
            heightmaps
                .motion_blocking_no_leaves
                .as_deref()
                .unwrap_or(&[0; 37]),
        )?;
    } else {
        let mut comp = pumpkin_nbt::compound::NbtCompound::new();
        if let Some(ref ws) = heightmaps.world_surface {
            comp.put(
                "WORLD_SURFACE",
                pumpkin_nbt::tag::NbtTag::LongArray(ws.to_vec()),
            );
        }
        if let Some(ref mb) = heightmaps.motion_blocking {
            comp.put(
                "MOTION_BLOCKING",
                pumpkin_nbt::tag::NbtTag::LongArray(mb.to_vec()),
            );
        }
        if let Some(ref mbnl) = heightmaps.motion_blocking_no_leaves {
            comp.put(
                "MOTION_BLOCKING_NO_LEAVES",
                pumpkin_nbt::tag::NbtTag::LongArray(mbnl.to_vec()),
            );
        }
        write_compound_nbt(&mut write, &comp, *version)?;
    }
    drop(heightmaps);

    {
        let mut blocks_and_biomes_buf = Vec::new();
        let block_sections = chunk
            .section
            .block_sections
            .read()
            .map_err(|_| WritingError::Message("block_sections read lock poisoned".into()))?;
        let biome_sections = chunk
            .section
            .biome_sections
            .read()
            .map_err(|_| WritingError::Message("biome_sections read lock poisoned".into()))?;

        let mut zero_bytes_count = 0;

        for (block_palette, biome_palette) in block_sections.iter().zip(biome_sections.iter()) {
            let non_empty_block_count = block_palette.non_air_block_count() as i16;
            blocks_and_biomes_buf.write_i16_be(non_empty_block_count)?;
            if version >= &JavaMinecraftVersion::V_26_1 {
                // New in 26.1, fluid count
                let liquid_count = block_palette.liquid_block_count() as i16;
                blocks_and_biomes_buf.write_i16_be(liquid_count)?;
            }

            let block_network = block_palette.convert_network();
            blocks_and_biomes_buf.write_u8(block_network.bits_per_entry)?;

            match block_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&block_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &block_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            let biome_network = biome_palette.convert_network();
            blocks_and_biomes_buf.write_u8(biome_network.bits_per_entry)?;

            match biome_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&biome_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &biome_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            if version == &JavaMinecraftVersion::V_1_21_5 {
                let block_storage_len = block_network.packed_data.len() as i32;
                let biome_storage_len = biome_network.packed_data.len() as i32;
                zero_bytes_count += VarInt(block_storage_len).written_size()
                    + VarInt(biome_storage_len).written_size();
            }
        }

        if version == &JavaMinecraftVersion::V_1_21_5 && zero_bytes_count > 0 {
            blocks_and_biomes_buf.resize(blocks_and_biomes_buf.len() + zero_bytes_count, 0);
        }

        write.write_var_int(&blocks_and_biomes_buf.len().try_into().map_err(|_| {
            WritingError::Message(format!(
                "{} is not representable as a VarInt!",
                blocks_and_biomes_buf.len()
            ))
        })?)?;
        write.write_slice(&blocks_and_biomes_buf)?;
    };

    let block_entities = chunk
        .pending_block_entities
        .lock()
        .map_err(|_| WritingError::Message("block_entities lock poisoned".into()))?;
    write.write_var_int(&VarInt(block_entities.len() as i32))?;
    for (pos, nbt) in block_entities.iter() {
        let local_xz = ((get_local_cord(pos.0.x) & 0xF) << 4) | (get_local_cord(pos.0.z) & 0xF);

        write.write_u8(local_xz as u8)?;
        write.write_i16_be(pos.0.y as i16)?;

        let id = nbt.get_string("id").map_or(0, |id_str| {
            let name = id_str.split(':').next_back().unwrap_or(id_str);
            pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
                .iter()
                .position(|&n| n == name)
                .unwrap_or(0)
        });
        write.write_var_int(&VarInt(id as i32))?;

        let mut client_nbt = nbt.clone();
        client_nbt.child_tags.remove("id");
        client_nbt.child_tags.remove("x");
        client_nbt.child_tags.remove("y");
        client_nbt.child_tags.remove("z");
        client_nbt.child_tags.remove("LootTable");
        client_nbt.child_tags.remove("LootTableSeed");
        client_nbt.child_tags.remove("PumpkinCustomData");
        client_nbt.child_tags.remove("BukkitValues");
        write_compound_nbt(&mut write, &client_nbt, *version)?;
    }

    {
        // Light masks include sections from -1 (below world) to num_sections (above world)
        // This means we need to account for 2 extra sections in the bitset
        let light_engine = chunk
            .light_engine
            .lock()
            .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;
        let num_sections = light_engine.sky_light.len();
        let sky_sent = light_engine
            .sky_light
            .iter()
            .rposition(|section| !is_filled(section, 0xFF))
            .map_or(0, |top| top + 1);
        let sky_kinds: Vec<LightKind> = light_engine
            .sky_light
            .iter()
            .enumerate()
            .map(|(index, section)| {
                if index >= sky_sent {
                    LightKind::Omitted
                } else {
                    LightKind::of(section)
                }
            })
            .collect();
        let block_kinds: Vec<LightKind> =
            light_engine.block_light.iter().map(LightKind::of).collect();

        let mut sky_light_empty_mask = 1u64;
        let mut block_light_empty_mask = 1u64;
        let mut sky_light_mask = 0u64;
        let mut block_light_mask = 0u64;

        for section_index in 0..num_sections {
            let bit = 1u64 << (section_index + 1);
            match sky_kinds[section_index] {
                LightKind::Data => sky_light_mask |= bit,
                LightKind::Empty => sky_light_empty_mask |= bit,
                LightKind::Omitted => {}
            }
            match block_kinds[section_index] {
                LightKind::Data => block_light_mask |= bit,
                LightKind::Empty => block_light_empty_mask |= bit,
                LightKind::Omitted => {}
            }
        }

        if sky_sent == num_sections {
            sky_light_empty_mask |= 1 << (num_sections + 1);
        }
        block_light_empty_mask |= 1 << (num_sections + 1);

        if version < &JavaMinecraftVersion::V_1_20 {
            write.write_bool(true)?;
        }

        BitSet(Box::new([sky_light_mask as i64])).encode_with_version(&mut write, version)?;
        BitSet(Box::new([block_light_mask as i64])).encode_with_version(&mut write, version)?;
        BitSet(Box::new([sky_light_empty_mask as i64])).encode_with_version(&mut write, version)?;
        BitSet(Box::new([block_light_empty_mask as i64]))
            .encode_with_version(&mut write, version)?;

        let light_data_size: VarInt = VarInt(LightContainer::ARRAY_SIZE as i32);

        write.write_var_int(&VarInt(sky_light_mask.count_ones() as i32))?;
        for (section, kind) in light_engine.sky_light.iter().zip(&sky_kinds) {
            if let (LightKind::Data, LightContainer::Full(data)) = (kind, section) {
                write.write_var_int(&light_data_size)?;
                write.write_slice(data.as_ref())?;
            }
        }

        write.write_var_int(&VarInt(block_light_mask.count_ones() as i32))?;
        for (section, kind) in light_engine.block_light.iter().zip(&block_kinds) {
            if let (LightKind::Data, LightContainer::Full(data)) = (kind, section) {
                write.write_var_int(&light_data_size)?;
                write.write_slice(data.as_ref())?;
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum LightKind {
    Data,
    Empty,
    Omitted,
}

impl LightKind {
    fn of(section: &LightContainer) -> Self {
        if matches!(section, LightContainer::Full(_)) && !is_filled(section, 0) {
            Self::Data
        } else {
            Self::Empty
        }
    }
}

fn is_filled(section: &LightContainer, byte: u8) -> bool {
    match section {
        LightContainer::Full(data) => data.iter().all(|&b| b == byte),
        LightContainer::Empty(level) => *level << 4 | *level == byte,
    }
}

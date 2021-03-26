use std::{
    collections::HashMap,
    io::{Read, Seek},
};

use crate::{Block, CCoord, Chunk, RCoord, Region, MIN_Y};

use super::{
    biome::{self, Biome},
    ChunkJava, RegionBuffer,
};

use log::debug;

pub type Rgba = [u8; 4];

/// ChunkRender objects can render a given chunk. What they render to is
/// entirely up to the implementation.
pub trait ChunkRender {
    /// Draw the given chunk.
    fn draw(&mut self, x: CCoord, z: CCoord, chunk: &dyn Chunk);

    /// Draw the invalid chunk. This means that the chunk was not of an expected
    /// form and couldn't be deserialized into a chunk object.
    fn draw_invalid(&mut self, x: CCoord, z: CCoord);
}

/// Palette can be used to take a block description to produce a colour that it
/// should render to.
pub trait Palette {
    fn pick(&self, block: &Block, biome: Option<Biome>) -> Rgba;
}

pub trait IntoMap {
    fn into_map(self) -> RegionMap<Rgba>;
}

pub struct RegionMap<T> {
    pub data: Vec<T>,
    pub x: RCoord,
    pub z: RCoord,
}

impl<T: Clone> RegionMap<T> {
    pub fn new(x: RCoord, z: RCoord, default: T) -> Self {
        let mut data: Vec<T> = Vec::new();
        data.resize(16 * 16 * 32 * 32, default);
        Self { data, x, z }
    }

    pub fn chunk_mut(&mut self, x: CCoord, z: CCoord) -> &mut [T] {
        debug_assert!(x.0 >= 0 && z.0 >= 0);

        let len = 16 * 16;
        let begin = (z.0 * 32 + x.0) as usize * len;
        &mut self.data[begin..begin + len]
    }

    pub fn chunk(&self, x: CCoord, z: CCoord) -> &[T] {
        debug_assert!(x.0 >= 0 && z.0 >= 0);

        let len = 16 * 16;
        let begin = (z.0 * 32 + x.0) as usize * len;
        &self.data[begin..begin + len]
    }
}

#[derive(Debug)]
pub enum DrawError {
    ParseAnvil(super::Error),
    ParseNbt(fastnbt::error::Error),
    IO(std::io::Error),
    MissingHeightMap,
    InvalidPalette,
}

impl From<super::Error> for DrawError {
    fn from(err: super::Error) -> DrawError {
        DrawError::ParseAnvil(err)
    }
}

impl From<fastnbt::error::Error> for DrawError {
    fn from(err: fastnbt::error::Error) -> DrawError {
        DrawError::ParseNbt(err)
    }
}

impl From<std::io::Error> for DrawError {
    fn from(err: std::io::Error) -> Self {
        DrawError::IO(err)
    }
}

pub type DrawResult<T> = std::result::Result<T, DrawError>;

pub fn parse_region<F: ChunkRender + ?Sized>(
    region: &dyn Region,
    draw_to: &mut F,
) -> DrawResult<()> {
    for x in 0isize..32 {
        for z in 0isize..32 {
            let (x, z) = (CCoord(x), CCoord(z));

            match region.chunk(x, z) {
                Some(chunk) => draw_to.draw(x, z, &*chunk),
                None => draw_to.draw_invalid(x, z),
            }
        }
    }

    Ok(())
}

pub struct RenderedPalette {
    pub blockstates: std::collections::HashMap<String, Rgba>,
    pub grass: image::RgbaImage,
    pub foliage: image::RgbaImage,
}

impl RenderedPalette {
    fn pick_grass(&self, b: Option<Biome>) -> Rgba {
        b.map(|b| {
            let climate = biome::climate(b);
            let t = climate.temperature.min(1.).max(0.);
            let r = climate.rainfall.min(1.).max(0.) * t;

            let t = 255 - (t * 255.).ceil() as u32;
            let r = 255 - (r * 255.).ceil() as u32;

            self.grass.get_pixel(t, r).0
        })
        .unwrap_or([255, 0, 0, 0])
    }

    fn pick_foliage(&self, b: Option<Biome>) -> Rgba {
        b.map(|b| {
            let climate = biome::climate(b);
            let t = climate.temperature.min(1.).max(0.);
            let r = climate.rainfall.min(1.).max(0.) * t;

            let t = 255 - (t * 255.).ceil() as u32;
            let r = 255 - (r * 255.).ceil() as u32;

            self.foliage.get_pixel(t, r).0
        })
        .unwrap_or([255, 0, 0, 0])
    }

    fn pick_water(&self, b: Option<Biome>) -> Rgba {
        b.map(|b| match b {
            Biome::Swamp => [0x61, 0x7B, 0x64, 255],
            Biome::River => [0x3F, 0x76, 0xE4, 255],
            Biome::Ocean => [0x3F, 0x76, 0xE4, 255],
            Biome::LukewarmOcean => [0x45, 0xAD, 0xF2, 255],
            Biome::WarmOcean => [0x43, 0xD5, 0xEE, 255],
            Biome::ColdOcean => [0x3D, 0x57, 0xD6, 255],
            Biome::FrozenRiver => [0x39, 0x38, 0xC9, 255],
            Biome::FrozenOcean => [0x39, 0x38, 0xC9, 255],
            _ => [0x3f, 0x76, 0xe4, 255],
        })
        .unwrap_or([0x3f, 0x76, 0xe4, 255])
    }
}

impl Palette for RenderedPalette {
    fn pick(&self, block: &Block, biome: Option<Biome>) -> Rgba {
        let missing_colour = [255, 0, 255, 255];
        let snow_block: Block = Block {
            name: "minecraft:snow_block".to_owned(),
            properties: HashMap::new(),
        };

        // A bunch of blocks in the game seem to be special cased outside of the
        // blockstate/model mechanism. For example leaves get coloured based on
        // the tree type and the biome type, but this is not encoded in the
        // blockstate or the model.
        //
        // This means we have to do a bunch of complex conditional logic in one
        // of the most called functions. Yuck.
        match block.name.strip_prefix("minecraft:") {
            Some(id) => {
                match id {
                    "grass" | "tall_grass" | "vine" | "fern" | "large_fern" => {
                        return self.pick_grass(biome);
                    }
                    "grass_block" => {
                        let snowy = block
                            .properties
                            .get("snowy")
                            .map(|s| *s == "true")
                            .unwrap_or(false);

                        if snowy {
                            return self.pick(&snow_block, biome);
                        } else {
                            return self.pick_grass(biome);
                        };
                    }
                    "water" | "bubble_column" => return self.pick_water(biome),
                    "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" => {
                        return self.pick_foliage(biome)
                    }
                    "birch_leaves" => {
                        return [0x80, 0xa7, 0x55, 255]; // game hardcodes this
                    }
                    "spruce_leaves" => {
                        return [0x61, 0x99, 0x61, 255]; // game hardcodes this
                    }
                    // Kelp and seagrass don't look like much from the top as
                    // they're flat. Maybe in future hard code a green tint to make
                    // it show up?
                    "kelp" | "seagrass" | "tall_seagrass" => {
                        return self.pick_water(biome);
                    }
                    "snow" => {
                        return self.pick(&snow_block, biome);
                    }
                    // Occurs a lot for the end, as layer 0 will be air in the void.
                    // Rendering it black makes sense in the end, but might look
                    // weird if it ends up elsewhere.
                    "air" => {
                        return [0, 0, 0, 255];
                    }
                    "cave_air" => {
                        return [255, 0, 0, 255]; // when does this happen??
                    }
                    // Otherwise fall through to the general mechanism.
                    _ => {}
                }
            }
            None => {}
        }

        let col = self
            .blockstates
            .get(&block.encoded_description())
            .or_else(|| self.blockstates.get(&block.name));

        match col {
            Some(c) => *c,
            None => {
                debug!("could not draw {}", block.name);
                debug!("description {}", block.encoded_description());

                missing_colour
            }
        }
    }
}

pub struct RegionBlockDrawer<'a, P: Palette + ?Sized> {
    pub map: RegionMap<Rgba>,
    pub palette: &'a P,
    pub processed_chunks: usize,
    pub painted_pixels: usize,
}

impl<'a, P: Palette + ?Sized> RegionBlockDrawer<'a, P> {
    pub fn new(map: RegionMap<Rgba>, palette: &'a P) -> Self {
        Self {
            map,
            palette,
            processed_chunks: 0,
            painted_pixels: 0,
        }
    }
}

impl<'a, P: Palette + ?Sized> IntoMap for RegionBlockDrawer<'a, P> {
    fn into_map(self) -> RegionMap<Rgba> {
        self.map
    }
}

impl<'a, P: Palette + ?Sized> ChunkRender for RegionBlockDrawer<'a, P> {
    fn draw(&mut self, x: CCoord, z: CCoord, chunk: &dyn Chunk) {
        let data = self.map.chunk_mut(x, z);
        self.processed_chunks += 1;

        if chunk.status() != "full" && chunk.status() != "spawn" {
            // Chunks that have been fully generated will have a 'full' status.
            // Skip chunks that don't; the way they render is unpredictable.
            return;
        }

        let mut draw_cross = false;

        for z in 0..16 {
            for x in 0..16 {
                let height = chunk.surface_height(x, z);

                let height = if height == MIN_Y { MIN_Y } else { height - 1 }; // -1 because we want the block below the air.
                let biome = chunk.biome(x, height, z);
                let block = chunk.block(x, height, z).unwrap(); // Block should definitely exist as we just figured out the height of it.

                let colour = self.palette.pick(&block, biome);

                let pixel = &mut data[z * 16 + x];
                *pixel = colour;
                self.painted_pixels += 1;
            }
        }

        if draw_cross {
            self.draw_invalid(x, z);
        }
    }

    fn draw_invalid(&mut self, x: CCoord, z: CCoord) {
        let data = self.map.chunk_mut(x, z);

        // Draw a red cross over the chunk
        for z in 0..16isize {
            for x in 0..16isize {
                let pixel = &mut data[z as usize * 16 + x as usize];

                *pixel = if (x - z).abs() < 3 || (x - (16 - z)).abs() < 3 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 0]
                }
            }
        }
    }
}

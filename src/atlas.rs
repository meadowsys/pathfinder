// Copyright 2017 The Servo Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use euclid::{Point2D, Rect, Size2D};
use gl::types::{GLenum, GLsizei, GLsizeiptr, GLuint, GLvoid};
use gl;
use outline::OutlineBuilder;
use rect_packer::RectPacker;
use std::mem;
use std::os::raw::c_void;
use std::u16;

pub struct AtlasBuilder {
    pub rect_packer: RectPacker,
    image_descriptors: Vec<ImageDescriptor>,
    image_metadata: Vec<ImageMetadata>,
}

impl AtlasBuilder {
    /// FIXME(pcwalton): Including the shelf height here may be a bad API.
    #[inline]
    pub fn new(available_width: u32, shelf_height: u32) -> AtlasBuilder {
        AtlasBuilder {
            rect_packer: RectPacker::new(available_width, shelf_height),
            image_descriptors: vec![],
            image_metadata: vec![],
        }
    }

    /// FIXME(pcwalton): Support the same glyph drawn at multiple point sizes.
    pub fn pack_glyph(&mut self,
                      outline_builder: &OutlineBuilder,
                      glyph_index: u32,
                      point_size: f32)
                      -> Result<(), ()> {
        // FIXME(pcwalton): I think this will check for negative values and panic, which is
        // unnecessary.
        let pixel_size = outline_builder.glyph_pixel_bounds(glyph_index, point_size)
                                        .size
                                        .ceil()
                                        .cast()
                                        .unwrap();

        let glyph_id = outline_builder.glyph_id(glyph_index);

        let atlas_origin = try!(self.rect_packer.pack(&pixel_size));

        let glyph_index = self.image_descriptors.len() as u32;

        while self.image_descriptors.len() < glyph_index as usize + 1 {
            self.image_descriptors.push(ImageDescriptor::default())
        }

        self.image_descriptors[glyph_index as usize] = ImageDescriptor {
            atlas_x: atlas_origin.x,
            atlas_y: atlas_origin.y,
            point_size: (point_size * 65536.0) as u32,
            glyph_index: glyph_index,
        };

        self.image_metadata.push(ImageMetadata {
            atlas_size: pixel_size,
            glyph_index: glyph_index,
            glyph_id: glyph_id,
        });

        Ok(())
    }

    pub fn create_atlas(&mut self, outline_builder: &OutlineBuilder) -> Result<Atlas, ()> {
        self.image_metadata.sort_by(|a, b| a.glyph_index.cmp(&b.glyph_index));

        let (mut current_range, mut counts, mut start_indices) = (None, vec![], vec![]);
        for image_metadata in &self.image_metadata {
            let glyph_index = image_metadata.glyph_index;

            let first_index = outline_builder.descriptors[glyph_index as usize]
                                             .start_index as usize;
            let last_index = match outline_builder.descriptors.get(glyph_index as usize + 1) {
                Some(ref descriptor) => descriptor.start_index as usize,
                None => outline_builder.indices.len(),
            };

            match current_range {
                Some((current_first, current_last)) if first_index == current_last => {
                    current_range = Some((current_first, last_index))
                }
                Some((current_first, current_last)) => {
                    counts.push((current_last - current_first) as GLsizei);
                    start_indices.push(current_first);
                    current_range = Some((first_index, last_index))
                }
                None => current_range = Some((first_index, last_index)),
            }
        }
        if let Some((current_first, current_last)) = current_range {
            counts.push((current_last - current_first) as GLsizei);
            start_indices.push(current_first);
        }

        // TODO(pcwalton): Try using `glMapBuffer` here.
        unsafe {
            let mut images = 0;
            gl::GenBuffers(1, &mut images);

            let length = self.image_descriptors.len() * mem::size_of::<ImageDescriptor>();
            let ptr = self.image_descriptors.as_ptr() as *const ImageDescriptor as *const c_void;
            gl::BindBuffer(gl::UNIFORM_BUFFER, images);
            gl::BufferData(gl::UNIFORM_BUFFER, length as GLsizeiptr, ptr, gl::DYNAMIC_DRAW);

            Ok(Atlas {
                start_indices: start_indices,
                counts: counts,
                images: images,

                shelf_height: self.rect_packer.shelf_height(),
                shelf_columns: self.rect_packer.shelf_columns(),
            })
        }
    }

    #[inline]
    pub fn glyph_index_for(&self, glyph_id: u16) -> Option<u32> {
        match self.image_metadata.binary_search_by(|metadata| metadata.glyph_id.cmp(&glyph_id)) {
            Ok(glyph_index) => Some(self.image_metadata[glyph_index].glyph_index),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn atlas_rect(&self, glyph_index: u32) -> Rect<u32> {
        let descriptor = &self.image_descriptors[glyph_index as usize];
        let metadata = &self.image_metadata[glyph_index as usize];
        Rect::new(Point2D::new(descriptor.atlas_x, descriptor.atlas_y), metadata.atlas_size)
    }
}

pub struct Atlas {
    start_indices: Vec<usize>,
    counts: Vec<GLsizei>,
    images: GLuint,

    pub shelf_height: u32,
    pub shelf_columns: u32,
}

impl Drop for Atlas {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteBuffers(1, &mut self.images);
        }
    }
}

impl Atlas {
    pub unsafe fn draw(&self, primitive: GLenum) {
        debug_assert!(self.counts.len() == self.start_indices.len());
        gl::MultiDrawElements(primitive,
                              self.counts.as_ptr(),
                              gl::UNSIGNED_INT,
                              self.start_indices.as_ptr() as *const *const GLvoid,
                              self.counts.len() as GLsizei);
    }

    #[inline]
    pub fn images(&self) -> GLuint {
        self.images
    }
}

/// Information about each image that we send to the GPU.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ImageDescriptor {
    atlas_x: u32,
    atlas_y: u32,
    point_size: u32,
    glyph_index: u32,
}

/// Information about each image that we keep around ourselves.
#[derive(Clone, Copy, Debug)]
pub struct ImageMetadata {
    atlas_size: Size2D<u32>,
    glyph_index: u32,
    glyph_id: u16,
}

use chrono::prelude::*;
use chrono::{DateTime, Duration, Utc};
use eframe::egui;
use image;
use image::GenericImageView;
use std::sync::{Arc, Condvar, Mutex};
use ureq;

const PARIS: (f32, f32) = (48.8575, 2.3514);
const TILES: ((u16, u16), (u16, u16)) = ((41, 61), (50, 68));

/// A helper function to load the image from bytes and create an egui texture.
fn load_image_from_memory(image_bytes: &[u8], name: &str, ctx: &egui::Context) -> Result<egui::TextureHandle, String> {
    // 1. Decode the image using the `image` crate.
    let image = image::load_from_memory_with_format(image_bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to decode PNG: {}", e))?;

    // 2. Convert the image to a format `egui` can use.
    let size = [image.width() as usize, image.height() as usize];
    let image_buffer = image.to_rgba8();
    let pixels = image_buffer.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

    // 3. Load the image into an `egui` texture.
    Ok(ctx.load_texture(name, color_image, Default::default()))
}

fn get_image(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    zoom: u16,
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
) -> Result<image::RgbImage, Box<dyn std::error::Error>> {
    let username = std::env::var("USER").unwrap();
    let standard_cache_folder =
        std::env::var("XDG_CACHE_HOME").unwrap_or(format!("/home/{}/.cache", username));
    let nuage_cache_folder = format!("{}/nuage/", standard_cache_folder);
    if !std::fs::exists(&nuage_cache_folder)? {
        std::fs::create_dir_all(&nuage_cache_folder)?;
    }

    let filepath = format!(
        "{}/{}{:0>2}{:0>2}{:0>2}{:0>2}_{}_{}_{}_{}_{}.jpg",
        &nuage_cache_folder, year, month, day, hour, minute, zoom, x1, y1, x2, y2,
    );

    if !std::fs::exists(&filepath)? {
        let url = format!(
            "https://imn-rust-lb.infoplaza.io/v4/nowcast/tiles/satellite-europe/{}{:0>2}{:0>2}{:0>2}{:0>2}/{}/{}/{}/{}/{}?outputtype=jpeg",
            year, month, day, hour, minute, zoom, x1, y1, x2, y2
        );
        println!("fetching {}", url);
        let mut res = ureq::get(url).call()?;
        let image_bytes = res
            .body_mut()
            .with_config()
            .limit(20 * 1024 * 1024)
            .read_to_vec()?;
        let img = image::load_from_memory(&image_bytes)?;
        let screen_width = 1920;
        let screen_height = 1080;
        let (width, height) = img.dimensions();
        let (new_width, new_height) = if width > screen_width || height > screen_height {
            let typical_screen_ratio = screen_width as f32 / screen_height as f32;
            let image_ratio = width as f32 / height as f32;
            if image_ratio < typical_screen_ratio {
                (
                    (width as f32 / (height as f32 / screen_height as f32)) as u32,
                    screen_height,
                )
            } else {
                (
                    screen_width,
                    (height as f32 / (width as f32 / screen_width as f32)) as u32,
                )
            }
        } else {
            (width, height)
        };
        let resized_img = img.resize(new_width, new_height, image::imageops::FilterType::Triangle);
        resized_img.save(&filepath)?;
    };
    println!("reading {}", filepath);
    let img = match image::ImageReader::open(filepath)?.decode()? {
        image::DynamicImage::ImageRgb8(rgb_image) => rgb_image,
        _ => return Err("Unsupported type of Jpeg".into()),
    };
    Ok(img)
}

fn convert_gps_to_pixels(_tiles: ((u16, u16), (u16, u16)), image_rect: &egui::Rect, _gps: (f32, f32)) -> (f32, f32) {
    // Stopgap while trying to figure out the coordinate system which does not
    // seem to follow slippy tiles.
    let center_x: f32 = image_rect.min.x + (image_rect.max.x - image_rect.min.x) / 2.;
    let center_y: f32 = image_rect.min.y + (image_rect.max.y - image_rect.min.y) / 2.;
    (center_x * 1.045 as f32, center_y * 0.68 as f32)
}

fn previous_time(now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let minute = now.minute();
    let to_five: u32 = minute - (minute as f32 / 5.) as u32 * 5;
    let now_at_five = now
        .checked_sub_signed(Duration::minutes(to_five as i64))
        .unwrap();

    let mut result = vec![];
    // can only access image older than 15 minutes
    let delay = 15;
    for x in (0..120).step_by(5) {
        let timepoint = now_at_five
            .checked_sub_signed(Duration::minutes(x + delay))
            .unwrap();
        result.push(timepoint);
    }
    result
}

struct SatImage {
    image: image::RgbImage,
    timestamp: DateTime<Utc>,
}

struct MyApp {
    // image: Result<egui::TextureHandle, String>,
    sat_images: Arc<(Mutex<Vec<SatImage>>, Condvar)>,
    image_index: usize,
    auto_play: bool,
    pinpoint_icon: egui::TextureHandle,
    downloading: Arc<Mutex<bool>>,
}

impl MyApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Self {
        // Add a custom font
        let font_bytes = include_bytes!("../VCR_OSD_MONO_1.001.ttf");
        // Load fonts
        let mut fonts = egui::FontDefinitions::default();
        // Install my own font
        fonts.font_data.insert(
            "vcr".to_owned(),
            egui::FontData::from_static(font_bytes).into(),
        );
        // Put my font first (highest priority):
        fonts
            .families
            .entry(egui::FontFamily::Name("vcr".into()))
            .or_default()
            .insert(0, "vcr".to_owned());
        // Tell egui to use the new `FontDefinitions`.
        cc.egui_ctx.set_fonts(fonts);

        // Build the time points use to create the image url
        let now = Utc::now();
        let timepoints = previous_time(now);
        let sat_images = Arc::new((Mutex::new(Vec::<SatImage>::new()), Condvar::new()));
        let sat_images_clone = sat_images.clone();
        let downloading = Arc::new(Mutex::new(true));
        let downloading_clone = downloading.clone();
        let ctx = Arc::new(cc.egui_ctx.clone());
        // Load the image in a separate thread
        std::thread::spawn(move || {
            for timepoint in timepoints {
                match get_image(
                    timepoint.year(),
                    timepoint.month(),
                    timepoint.day(),
                    timepoint.hour(),
                    timepoint.minute(),
                    7,
                    TILES.0.0,
                    TILES.0.1,
                    TILES.1.0,
                    TILES.1.1,
                ) {
                    Ok(image) => {
                        let (images, cvar) = &*sat_images;
                        let mut images = images.lock().unwrap();
                        images.push(SatImage {
                            image,
                            timestamp: timepoint,
                        });
                        cvar.notify_one();
                        ctx.request_repaint();
                    }
                    _ => {}
                }
            }
            *downloading.lock().unwrap() = false;
        });

        Self {
            image_index: 0,
            sat_images: sat_images_clone,
            auto_play: true,
            pinpoint_icon: load_image_from_memory(
                include_bytes!("../pinpoint-icon.png"),
                "pinpoint_icon", &cc.egui_ctx).expect("Could not load pinpoint"),
            downloading: downloading_clone,
        }
    }

    fn increase_image_index(image_index: &mut usize, nb_images: usize) {
        if *image_index == nb_images - 1 {
            *image_index = 0;
        } else {
            *image_index += 1;
        }
    }

    fn decrease_image_index(image_index: &mut usize, nb_images: usize) {
        if *image_index == 0 {
            *image_index = nb_images - 1;
        } else {
            *image_index -= 1;
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check if the user has pressed the Escape key.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // If so, tell the frame to close.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Get the lock on the images
        let (sat_images, cvar) = &*self.sat_images;
        let mut sat_images = sat_images.lock().unwrap();
        // Check we have images
        if sat_images.len() == 0 {
            // Wait for images
            while sat_images.len() == 0 {
                sat_images = cvar.wait(sat_images).unwrap();
            }
        }
        let time = ctx.input(|i| i.time);
        if self.auto_play {
            // Let's say an image every 1/5th of a second
            let cycle_duration = sat_images.len() as f64 / 5.;
            let time_in_cycle = time % cycle_duration;
            self.image_index = sat_images.len() - 1 - (time_in_cycle * sat_images.len() as f64 / cycle_duration) as usize;
            ctx.request_repaint();
        }
        // Images are order from the most recent to the least.
        // Index 0 is the most recent.
        // Navigate the image with left...
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.auto_play = false;
            MyApp::decrease_image_index(&mut self.image_index, sat_images.len());
        }
        // ... and right.
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.auto_play = false;
            MyApp::increase_image_index(&mut self.image_index, sat_images.len());
        }
        // Pause / Unpaause on space
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.auto_play = !self.auto_play;
        }

        let sat_image = &sat_images[self.image_index];
        let dimensions = sat_image.image.dimensions();
        let color_image = egui::ColorImage::from_rgb(
            [dimensions.0 as usize, dimensions.1 as usize],
            &sat_image.image.as_raw(),
        );
        let texture_handle = ctx.load_texture("my-jpeg-image", color_image, Default::default());

        // Blinking download label
        const BLINK_HZ: f64 = 2.0;
        let cycle_duration = 1.0 / BLINK_HZ;
        let time_in_cycle = time % cycle_duration;
        let downloading_is_visible = time_in_cycle < (cycle_duration / 2.0);

        let mut displayed_image_rect: Option<egui::Rect> = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                // This code here center the image but we loose its actual position in pixel
                // let available_size = ui.available_size();
                // ui.add(egui::Image::new(&texture_handle).max_size(available_size));
                // We center is ourselves here so we keep its exact position
                let available_rect = ui.available_rect_before_wrap();
                let image_size = texture_handle.size_vec2();
                let aspect_ratio = image_size.x / image_size.y;
                let mut target_size = available_rect.size();
                if target_size.x / target_size.y > aspect_ratio {
                    // The container is wider than the image.
                    target_size.x = target_size.y * aspect_ratio;
                } else {
                    // The container is taller than the image.
                    target_size.y = target_size.x / aspect_ratio;
                }
                let image_rect = egui::Rect::from_center_size(available_rect.center(), target_size);
                let response = ui.allocate_rect(image_rect, egui::Sense::hover());
                if ui.is_rect_visible(response.rect) {
                    let mut mesh = egui::Mesh::with_texture(texture_handle.id());
                    mesh.add_rect_with_uv(response.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                    ui.painter().add(egui::Shape::mesh(mesh));
                }
                displayed_image_rect = Some(response.rect);

            });
            // Top-left corner for the header.
            egui::Area::new("header_area".into())
                .anchor(egui::Align2::LEFT_TOP, egui::Vec2::new(10.0, 10.0)) // Anchor with a 10px margin.
                .show(ctx, |ui| {
                    ui.heading("Nuage (Press ESC to exit)");
                });
            // Bottom-left corner for the image detail label
            egui::Area::new("custom_label_area".into())
                .anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::new(10.0, -10.0)) // Anchor with a 10px margin.
                .show(ctx, |ui| {
                    let local_timestamp: DateTime<Local> = DateTime::from(sat_image.timestamp);
                    let custom_label = egui::RichText::new(format!(
                        "{:0>2}/{:0>2} {:0>2}-{:0>2}-{} {:0>2}:{:0>2}",
                        // as image are order from most recent to least recent,
                        // we display here a more natural index
                        sat_images.len() - self.image_index,
                        sat_images.len(),
                        local_timestamp.day(),
                        local_timestamp.month(),
                        local_timestamp.year(),
                        local_timestamp.hour(),
                        local_timestamp.minute()
                    ))
                    .font(egui::FontId::new(
                        24.0,
                        egui::FontFamily::Name("vcr".into()),
                    ))
                    .color(egui::Color32::WHITE) // Make it visible on a dark image
                    .background_color(egui::Color32::TRANSPARENT); // Semi-transparent background

                    ui.add(egui::Label::new(custom_label).extend());
                });

            if *self.downloading.lock().unwrap() && downloading_is_visible {
                // Bottom-left corner for the image detail label
                egui::Area::new("downloading_area".into())
                    .anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::new(10.0, -40.0)) // Anchor with a 10px margin.
                    .show(ctx, |ui| {
                        let custom_label = egui::RichText::new("DOWNLOADING...")
                            .font(egui::FontId::new(
                                24.0,
                                egui::FontFamily::Name("vcr".into()),
                            ))
                            .color(egui::Color32::WHITE) // Make it visible on a dark image
                            .background_color(egui::Color32::TRANSPARENT); // Semi-transparent background

                        ui.add(egui::Label::new(custom_label).extend());
                    });
            }

            // Pinpoint icon
            let point_of_interest = convert_gps_to_pixels(TILES, &displayed_image_rect.unwrap(), PARIS);
            egui::Area::new("pinpoint_area".into())
                .fixed_pos(egui::pos2(
                    point_of_interest.0 as f32 - self.pinpoint_icon.size()[0] as f32 / 2.,
                    point_of_interest.1 as f32 - self.pinpoint_icon.size()[1] as f32,
                )) // The top-left corner of the Area
                // .fixed_pos(egui::pos2(
                //     point_of_interest.0,
                //     point_of_interest.1,
                // )) // The top-left corner of the Area
                .show(ctx, |ui| {
                    ui.image(&self.pinpoint_icon);
                });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Nuage",
        options,
        Box::new(|cc| Ok(Box::new(MyApp::new(cc)))),
    )
}

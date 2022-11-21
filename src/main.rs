use array2d::Array2D;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use freenectrs::freenect::FreenectDepthStream;
use freenectrs::freenect::{self, FreenectDevice};

struct Kinect<'a> {
    dstream: FreenectDepthStream<'a, 'a>,
    device: &'a FreenectDevice<'a, 'a>,
}

#[derive(Component)]
struct CurrentDepth<'a> {
    depth_array: &'a [u16],
    handle: Handle<Image>,
}

#[derive(Component)]
struct Crosshair;

#[derive(Component)]
struct MainCamera;

fn setup_kinect(world: &mut World) {
    let ctx = Box::leak(Box::new(
        freenect::FreenectContext::init_with_video_motor().unwrap(),
    ));

    let dev_count = ctx.num_devices().unwrap();
    if dev_count == 0 {
        eprintln!("No device connected - abort");
    } else {
        println!("Found {} devices, use first", dev_count);
    }

    let device = Box::leak(Box::new(ctx.open_device(0).unwrap()));

    device
        .set_depth_mode(
            freenect::FreenectResolution::Medium,
            freenect::FreenectDepthFormat::Bit10,
        )
        .unwrap();

    let dstream = device.depth_stream().unwrap();

    ctx.spawn_process_thread().unwrap();

    world.insert_non_send_resource(Kinect { dstream, device });
}

fn spawn_depth(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    asset_server: Res<AssetServer>,
) {
    commands
        .spawn(SpriteBundle {
            texture: asset_server.load("crosshair.png"),
            ..default()
        })
        .insert(Crosshair);

    commands.spawn(Camera2dBundle::default()).insert(MainCamera);

    let image_handle = images.add(Image::new_fill(
        Extent3d {
            width: 640,
            height: 480,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8Unorm,
    ));

    commands.spawn_empty().insert(CurrentDepth {
        depth_array: &[],
        handle: image_handle.clone(),
    });

    commands
        .spawn(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            ..default()
        })
        .with_children(|parent| {
            parent
                .spawn(NodeBundle {
                    style: Style {
                        size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                        position_type: PositionType::Absolute,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::FlexStart,
                        ..default()
                    },
                    ..default()
                })
                .with_children(|parent| {
                    // bevy logo (image)
                    parent.spawn(ImageBundle {
                        style: Style {
                            size: Size::new(Val::Px(640.0), Val::Px(480.0)),
                            ..default()
                        },
                        image: UiImage(image_handle),
                        ..default()
                    });
                });
        });
}

fn read_depth_data(kinect: NonSend<Kinect>, mut depth_query: Query<&mut CurrentDepth<'static>>) {
    let depth_res = depth_query.get_single_mut();

    match depth_res {
        Ok(mut depth) => {
            if let Ok((data, _ /* timestamp */)) = kinect.dstream.receiver.try_recv() {
                depth.depth_array = data;
                return;
            } else {
                return;
            }
        }
        Err(_) => {}
    }
}

fn update_image_from_depth_data(
    depth_query: Query<&CurrentDepth<'static>>,
    mut images: ResMut<Assets<Image>>,
) {
    let depth_res = depth_query.get_single();

    match depth_res {
        Ok(depth) => {
            if depth.depth_array.len() == 0 {
                return;
            }
            if let Some(mut handle) = images.get_mut(&depth.handle) {
                let mut new_pixels: Vec<u8> = vec![];

                for measurement in depth.depth_array.iter() {
                    new_pixels.push(0);
                    new_pixels.push(0);
                    new_pixels.push(0);
                    new_pixels.push((measurement / 8) as u8);
                }

                handle.data = new_pixels;
            }
        }
        Err(_) => {}
    }
}

fn move_crosshair_to_pos(
    depth_query: Query<&CurrentDepth<'static>>,
    mut transform_query: Query<&mut Transform, With<Crosshair>>,
    q_camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    if let Ok(depth) = depth_query.get_single() {
        if depth.depth_array.len() == 0 {
            return;
        }
        let (camera, camera_transform) = q_camera.single();

        let mut screen_pos = center_of_close_blob(depth.depth_array);
        screen_pos.y = (screen_pos.y - 480.0).abs();
        if screen_pos.x < 0.1 {
            return;
        }

        let window_size = Vec2::new(640.0 as f32, 480.0 as f32);

        // convert screen position [0..resolution] to ndc [-1..1] (gpu coordinates)
        let ndc = (screen_pos / window_size) * 2.0 - Vec2::ONE;

        // matrix for undoing the projection and camera transform
        let ndc_to_world = camera_transform.compute_matrix() * camera.projection_matrix().inverse();

        // use it to convert ndc to world-space coordinates
        let world_pos = ndc_to_world.project_point3(ndc.extend(-1.0));

        // reduce it to a 2D value
        let world_pos: Vec2 = world_pos.truncate();

        let mut crosshair_t = transform_query.single_mut();
        crosshair_t.translation.x = world_pos.x;
        crosshair_t.translation.y = world_pos.y;
    }
}

fn center_of_close_blob(data: &[u16]) -> Vec2 {
    // assumes 640 x 480

    let mut break_outer = false;

    let mut left_most: u16 = 0;
    let mut right_most: u16 = 0;
    let mut top_most: u16 = 0;
    let mut bottom_most: u16 = 0;

    let arr_2d = Array2D::from_iter_row_major(data.iter(), 480, 640);
    for i in 0..640 {
        for k in arr_2d.column_iter(i) {
            if k < &&400 {
                break_outer = true;
                left_most = i as u16;
                break;
            }
        }
        if break_outer {
            break;
        }
    }

    break_outer = false;

    let arr_2d = Array2D::from_iter_row_major(data.iter(), 480, 640);
    for i in (0..640).rev() {
        for k in arr_2d.column_iter(i) {
            if k < &&400 {
                break_outer = true;
                right_most = i as u16;
                break;
            }
        }
        if break_outer {
            break;
        }
    }

    break_outer = false;

    let arr_2d = Array2D::from_iter_row_major(data.iter(), 480, 640);
    for i in 0..480 {
        for k in arr_2d.row_iter(i) {
            if k < &&400 {
                break_outer = true;
                top_most = i as u16;
                break;
            }
        }
        if break_outer {
            break;
        }
    }

    break_outer = false;

    let arr_2d = Array2D::from_iter_row_major(data.iter(), 480, 640);
    for i in (0..480).rev() {
        for k in arr_2d.row_iter(i) {
            if k < &&400 {
                break_outer = true;
                bottom_most = i as u16;
                break;
            }
        }
        if break_outer {
            break;
        }
    }

    Vec2::new(
        ((left_most + right_most) / 2).into(),
        ((top_most + bottom_most) / 2).into(),
    )
}

fn keyboard_input(keys: Res<Input<KeyCode>>, kinect: NonSend<Kinect>) {
    if keys.just_pressed(KeyCode::Down) {
        let tilt_degree = kinect.device.get_tilt_degree().unwrap();
        kinect.device.set_tilt_degree(tilt_degree - 5.0).unwrap();
    }

    if keys.just_pressed(KeyCode::Up) {
        let tilt_degree = kinect.device.get_tilt_degree().unwrap();
        kinect.device.set_tilt_degree(tilt_degree + 5.0).unwrap();
    }
}

fn main() {
    App::new()
        .add_startup_system(setup_kinect)
        .add_startup_system(spawn_depth)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            window: WindowDescriptor {
                title: "Bevy Kinect".to_string(),
                width: 640.,
                height: 480.,
                ..default()
            },
            ..default()
        }))
        .add_system(read_depth_data)
        .add_system(keyboard_input)
        .add_system(update_image_from_depth_data)
        .add_system(move_crosshair_to_pos)
        .run();
}

use embedded_graphics::{
    geometry::Point,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, Triangle},
};
use epd_waveshare::{
    color::Black,
    epd2in7b::{Display2in7b, Epd2in7b},
    graphics::{Display, DisplayRotation},
    prelude::*,
};
use linux_embedded_hal::{
    spidev::{self, SpidevOptions},
    sysfs_gpio::Direction,
    Delay, Pin, Spidev,
};
use rppal::gpio::Gpio;
use rppal::gpio::Level;
use rppal::gpio::Trigger;
use std::sync::{mpsc, Arc, Mutex};

struct Arrow {
    pub x: i32,
    pub y: i32,
    pub radius: i32,
    pub rotation: DisplayRotation,
}

impl Arrow {
    fn new(radius: i32) -> Self {
        Self {
            radius,
            x: radius,
            y: radius,
            rotation: DisplayRotation::Rotate0,
        }
    }

    fn draw(&self, display: &mut Display2in7b) {
        display.clear_buffer(Color::White);

        let rect_size = Size::new(self.radius as u32, self.radius as u32);
        let (rectangle, triangle) = match self.rotation {
            DisplayRotation::Rotate0 => (
                Rectangle::new(
                    Point::new(self.x - (self.radius / 2), self.y - self.radius),
                    rect_size,
                ),
                Triangle::new(
                    Point::new(self.x - self.radius, self.y),
                    Point::new(self.x, self.y + self.radius),
                    Point::new(self.x + self.radius, self.y),
                ),
            ),
            DisplayRotation::Rotate90 => (
                Rectangle::new(Point::new(self.x, self.y - (self.radius / 2)), rect_size),
                Triangle::new(
                    Point::new(self.x, self.y - self.radius),
                    Point::new(self.x - self.radius, self.y),
                    Point::new(self.x, self.y + self.radius),
                ),
            ),
            DisplayRotation::Rotate180 => (
                Rectangle::new(Point::new(self.x - (self.radius / 2), self.y), rect_size),
                Triangle::new(
                    Point::new(self.x - self.radius, self.y),
                    Point::new(self.x, self.y - self.radius),
                    Point::new(self.x + self.radius, self.y),
                ),
            ),
            DisplayRotation::Rotate270 => (
                Rectangle::new(
                    Point::new(self.x - self.radius, self.y - (self.radius / 2)),
                    rect_size,
                ),
                Triangle::new(
                    Point::new(self.x, self.y - self.radius),
                    Point::new(self.x + self.radius, self.y),
                    Point::new(self.x, self.y + self.radius),
                ),
            ),
        };
        let _ = rectangle
            .into_styled(PrimitiveStyle::with_fill(Black))
            .draw(display);
        let _ = triangle
            .into_styled(PrimitiveStyle::with_fill(Black))
            .draw(display);
    }

    fn rotate(&mut self) {
        self.rotation = match self.rotation {
            DisplayRotation::Rotate0 => DisplayRotation::Rotate90,
            DisplayRotation::Rotate90 => DisplayRotation::Rotate180,
            DisplayRotation::Rotate180 => DisplayRotation::Rotate270,
            DisplayRotation::Rotate270 => DisplayRotation::Rotate0,
        }
    }

    fn move_forward(&mut self, distance: i32) {
        match self.rotation {
            DisplayRotation::Rotate0 => self.y += distance,
            DisplayRotation::Rotate90 => self.x -= distance,
            DisplayRotation::Rotate180 => self.y -= distance,
            DisplayRotation::Rotate270 => self.x += distance,
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum ArrowMessage {
    Rotate,
    MoveForward(i32),
}

// activate spi, gpio in raspi-config
// needs to be run with sudo because of some sysfs_gpio permission problems and follow-up timing problems
// see https://github.com/rust-embedded/rust-sysfs-gpio/issues/5 and follow-up issues
// https://github.com/rust-embedded/rust-sysfs-gpio/issues/24
// https://github.com/golemparts/rppal/issues/41

fn main() -> Result<(), std::io::Error> {
    // Configure SPI
    let mut spi = Spidev::open("/dev/spidev0.0").expect("spidev directory");
    let options = SpidevOptions::new()
        .bits_per_word(8)
        .max_speed_hz(4_000_000)
        .mode(spidev::SpiModeFlags::SPI_MODE_0)
        .build();
    spi.configure(&options).expect("spi configuration");

    // Configure Digital I/O Pin to be used as Chip Select for SPI
    let cs = Pin::new(5); //BCM7 CE0
    cs.export().expect("cs export");
    while !cs.is_exported() {}
    cs.set_direction(Direction::Out).expect("CS Direction");
    cs.set_value(1).expect("CS Value set to 1");

    let busy = Pin::new(19); //pin 29
    busy.export().expect("busy export");
    while !busy.is_exported() {}
    busy.set_direction(Direction::In).expect("busy Direction");

    let dc = Pin::new(6); //pin 31 //bcm6
    dc.export().expect("dc export");
    while !dc.is_exported() {}
    dc.set_direction(Direction::Out).expect("dc Direction");
    dc.set_value(1).expect("dc Value set to 1");

    let rst = Pin::new(13); //pin 36 //bcm16
    rst.export().expect("rst export");
    while !rst.is_exported() {}
    rst.set_direction(Direction::Out).expect("rst Direction");
    rst.set_value(1).expect("rst Value set to 1");

    let mut delay = Delay {};

    let mut epd2in7b =
        Epd2in7b::new(&mut spi, cs, busy, dc, rst, &mut delay).expect("eink initalize error");
    println!("Initialized");

    let mut display = Display2in7b::default();
    let mut arrow = Arrow::new(20);

    display.clear_buffer(Color::White);
    epd2in7b.clear_frame(&mut spi, &mut delay)?;

    arrow.draw(&mut display);

    epd2in7b.update_frame(&mut spi, display.buffer(), &mut delay)?;
    epd2in7b
        .display_frame(&mut spi, &mut delay)
        .expect("displaying");

    let gpio = Gpio::new().expect("Gpio new");
    // closest to ethernet
    let move_button = gpio.get(20).expect("btn 1");
    // furthest from output
    let rotate_button = gpio.get(21).expect("btn 2");

    let mut move_button_pin = move_button.into_input_pullup();
    let mut rotate_button_pin = rotate_button.into_input_pullup();

    let arrow_mutex = Arc::new(Mutex::new(arrow));

    let (tx, rx) = mpsc::channel();
    let rotate_tx = tx.clone();

    move_button_pin
        .set_async_interrupt(Trigger::FallingEdge, move |level: Level| {
            println!("Btn 1 pushed: {}", level);
            if let Level::Low = level {
                tx.send(ArrowMessage::MoveForward(100)).unwrap();
            }
        })
        .unwrap();
    rotate_button_pin
        .set_async_interrupt(Trigger::FallingEdge, move |level: Level| {
            println!("Btn 2 pushed: {}", level);
            if let Level::Low = level {
                rotate_tx.send(ArrowMessage::Rotate).unwrap();
            }
        })
        .unwrap();

    println!("Waiting for input");

    for received in rx {
        println!(
            "button 1 (move): {}, button 2 (rotate): {}",
            move_button_pin.read(),
            rotate_button_pin.read()
        );
        let mut arrow = arrow_mutex.lock().unwrap();
        match received {
            ArrowMessage::MoveForward(distance) => arrow.move_forward(distance),
            ArrowMessage::Rotate => arrow.rotate(),
        }
        arrow.draw(&mut display);
        epd2in7b.update_frame(&mut spi, display.buffer(), &mut delay)?;
        epd2in7b
            .display_frame(&mut spi, &mut delay)
            .expect("displaying");
    }

    // TODO: Handle interrupt
    println!("Finished, going to sleep");
    epd2in7b.sleep(&mut spi, &mut delay)
}

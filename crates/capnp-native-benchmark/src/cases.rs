use std::cmp::Ordering;
use std::sync::Arc;

use capnp_message::{ExclusiveArena, OwnedMessage};
use capnp_schema::CompiledSchema;

use crate::carsales::{Color, car, parking_lot, total_value, wheel};
use crate::catrank::{search_result, search_result_list};
use crate::common::{BenchResult, Case, FastRand, WORDS, new_arena, safe_divide, safe_modulus};
use crate::eval::{Operation, evaluation_result, expression, left, right};

pub struct CarSales;

impl Case for CarSales {
    type Expectation = u64;

    fn build_request(
        schema: &CompiledSchema,
        arena: &mut ExclusiveArena,
        random: &mut FastRand,
    ) -> BenchResult<Self::Expectation> {
        let mut request = parking_lot::Builder::init_root(schema, arena)?;
        let count = random.next_less_than(200);
        let mut cars = request.init_cars(count)?;
        let mut total = 0_u64;
        for index in 0..count {
            let mut value = car::Builder::from_dynamic(cars.struct_element(index)?);
            total += random_car(random, &mut value)?;
        }
        Ok(total)
    }

    fn handle_request(
        schema: &Arc<CompiledSchema>,
        request: Arc<OwnedMessage>,
    ) -> BenchResult<ExclusiveArena> {
        let request = parking_lot::Reader::from_root(Arc::clone(schema), request)?;
        let cars = request.cars()?.ok_or("cars list is null")?;
        let mut total = 0_u64;
        for index in 0..cars.len()? {
            total += car_value(&cars.get(index)?)?;
        }
        let mut response = new_arena()?;
        total_value::Builder::init_root(schema, &mut response)?.set_amount(total)?;
        Ok(response)
    }

    fn check_response(
        schema: &Arc<CompiledSchema>,
        response: Arc<OwnedMessage>,
        expected: Self::Expectation,
    ) -> BenchResult<bool> {
        Ok(total_value::Reader::from_root(Arc::clone(schema), response)?.amount()? == expected)
    }
}

fn random_car(random: &mut FastRand, car: &mut car::Builder<'_, '_>) -> BenchResult<u64> {
    const MAKES: [&str; 5] = ["Toyota", "GM", "Ford", "Honda", "Tesla"];
    const MODELS: [&str; 6] = ["Camry", "Prius", "Volt", "Accord", "Leaf", "Model S"];
    car.set_make(MAKES[random.next_less_than(MAKES.len() as u32) as usize])?;
    car.set_model(MODELS[random.next_less_than(MODELS.len() as u32) as usize])?;
    car.set_color(Color::from_ordinal(random.next_less_than(9) as u16))?;
    let seats = 2 + random.next_less_than(6) as u8;
    let doors = 2 + random.next_less_than(3) as u8;
    car.set_seats(seats)?;
    car.set_doors(doors)?;

    let mut wheel_values = [(0_u16, false); 4];
    {
        let mut wheels = car.init_wheels(4)?;
        for (index, item) in wheel_values.iter_mut().enumerate() {
            let mut wheel = wheel::Builder::from_dynamic(wheels.struct_element(index as u32)?);
            let diameter = 25 + random.next_less_than(15) as u16;
            let snow_tires = random_wheel(random, &mut wheel, diameter)?;
            *item = (diameter, snow_tires);
        }
    }

    let length = 170 + random.next_less_than(150) as u16;
    let width = 48 + random.next_less_than(36) as u16;
    let height = 54 + random.next_less_than(48) as u16;
    car.set_length(length)?;
    car.set_width(width)?;
    car.set_height(height)?;
    car.set_weight(u32::from(length) * u32::from(width) * u32::from(height) / 200)?;

    let horsepower = 100 * random.next_less_than(400) as u16;
    let uses_electric = {
        let mut engine = car.init_engine()?;
        engine.set_horsepower(horsepower)?;
        engine.set_cylinders(4 + 2 * random.next_less_than(3) as u8)?;
        engine.set_cc(800 + random.next_less_than(10_000))?;
        engine.set_uses_gas(true)?;
        let uses_electric = random.next_bool();
        engine.set_uses_electric(uses_electric)?;
        uses_electric
    };

    let fuel_capacity = (10.0 + random.next_double(30.0)) as f32;
    car.set_fuel_capacity(fuel_capacity)?;
    car.set_fuel_level(random.next_double(f64::from(fuel_capacity)) as f32)?;
    let power_windows = random.next_bool();
    let power_steering = random.next_bool();
    let cruise_control = random.next_bool();
    let cup_holders = random.next_less_than(12) as u8;
    let nav_system = random.next_bool();
    car.set_has_power_windows(power_windows)?;
    car.set_has_power_steering(power_steering)?;
    car.set_has_cruise_control(cruise_control)?;
    car.set_cup_holders(cup_holders)?;
    car.set_has_nav_system(nav_system)?;

    let mut total = u64::from(seats) * 200 + u64::from(doors) * 350;
    for (diameter, snow_tires) in wheel_values {
        total += u64::from(diameter) * u64::from(diameter);
        total += if snow_tires { 100 } else { 0 };
    }
    total += u64::from(length) * u64::from(width) * u64::from(height) / 50;
    total += u64::from(horsepower) * 40;
    if uses_electric {
        total += 5000;
    }
    total += if power_windows { 100 } else { 0 };
    total += if power_steering { 200 } else { 0 };
    total += if cruise_control { 400 } else { 0 };
    total += if nav_system { 2000 } else { 0 };
    total += u64::from(cup_holders) * 25;
    Ok(total)
}

fn random_wheel(
    random: &mut FastRand,
    wheel: &mut wheel::Builder<'_, '_>,
    diameter: u16,
) -> BenchResult<bool> {
    wheel.set_diameter(diameter)?;
    wheel.set_air_pressure((30.0 + random.next_double(20.0)) as f32)?;
    let snow_tires = random.next_less_than(16) == 0;
    wheel.set_snow_tires(snow_tires)?;
    Ok(snow_tires)
}

fn car_value(car: &car::Reader) -> BenchResult<u64> {
    let mut total = u64::from(car.seats()?) * 200 + u64::from(car.doors()?) * 350;
    let wheels = car.wheels()?.ok_or("wheels list is null")?;
    for index in 0..wheels.len()? {
        let wheel = wheels.get(index)?;
        let diameter = u64::from(wheel.diameter()?);
        total += diameter * diameter;
        total += if wheel.snow_tires()? { 100 } else { 0 };
    }
    total += u64::from(car.length()?) * u64::from(car.width()?) * u64::from(car.height()?) / 50;
    let engine = car.engine()?.ok_or("engine is null")?;
    total += u64::from(engine.horsepower()?) * 40;
    if engine.uses_electric()? {
        total += if engine.uses_gas()? { 5000 } else { 3000 };
    }
    total += if car.has_power_windows()? { 100 } else { 0 };
    total += if car.has_power_steering()? { 200 } else { 0 };
    total += if car.has_cruise_control()? { 400 } else { 0 };
    total += if car.has_nav_system()? { 2000 } else { 0 };
    total += u64::from(car.cup_holders()?) * 25;
    Ok(total)
}

pub struct CatRank;

impl Case for CatRank {
    type Expectation = u32;

    fn build_request(
        schema: &CompiledSchema,
        arena: &mut ExclusiveArena,
        random: &mut FastRand,
    ) -> BenchResult<Self::Expectation> {
        let count = random.next_less_than(1000);
        let mut request = search_result_list::Builder::init_root(schema, arena)?;
        let mut results = request.init_results(count)?;
        let mut good_count = 0_u32;
        for index in 0..count {
            let mut result = search_result::Builder::from_dynamic(results.struct_element(index)?);
            result.set_score(f64::from(1000 - index))?;
            let url_size = random.next_less_than(100);
            let mut url = String::from("http://example.com/");
            for _ in 0..url_size {
                url.push(char::from(b'a' + random.next_less_than(26) as u8));
            }
            result.set_url(&url)?;
            let is_cat = random.next_less_than(8) == 0;
            let is_dog = random.next_less_than(8) == 0;
            good_count += u32::from(is_cat && !is_dog);
            let mut snippet = String::from(" ");
            let prefix = random.next_less_than(20);
            append_words(random, &mut snippet, prefix);
            if is_cat {
                snippet.push_str("cat ");
            }
            if is_dog {
                snippet.push_str("dog ");
            }
            let suffix = random.next_less_than(20);
            append_words(random, &mut snippet, suffix);
            result.set_snippet(&snippet)?;
        }
        Ok(good_count)
    }

    fn handle_request(
        schema: &Arc<CompiledSchema>,
        request: Arc<OwnedMessage>,
    ) -> BenchResult<ExclusiveArena> {
        let request = search_result_list::Reader::from_root(Arc::clone(schema), request)?;
        let results = request.results()?.ok_or("search results list is null")?;
        let mut scored = Vec::with_capacity(results.len()? as usize);
        for index in 0..results.len()? {
            let result = results.get(index)?;
            let snippet = result.snippet()?;
            let mut score = result.score()?;
            if snippet.contains(" cat ") {
                score *= 10_000.0;
            }
            if snippet.contains(" dog ") {
                score /= 10_000.0;
            }
            scored.push((score, result));
        }
        scored.sort_unstable_by(|left, right| {
            right.0.partial_cmp(&left.0).unwrap_or(Ordering::Equal)
        });

        let mut response = new_arena()?;
        let mut root = search_result_list::Builder::init_root(schema, &mut response)?;
        let mut output = root.init_results(u32::try_from(scored.len())?)?;
        for (index, (score, source)) in scored.into_iter().enumerate() {
            let mut destination =
                search_result::Builder::from_dynamic(output.struct_element(index as u32)?);
            destination.set_score(score)?;
            destination.set_url(&source.url()?)?;
            destination.set_snippet(&source.snippet()?)?;
        }
        Ok(response)
    }

    fn check_response(
        schema: &Arc<CompiledSchema>,
        response: Arc<OwnedMessage>,
        expected: Self::Expectation,
    ) -> BenchResult<bool> {
        let response = search_result_list::Reader::from_root(Arc::clone(schema), response)?;
        let results = response.results()?.ok_or("search results list is null")?;
        let mut count = 0_u32;
        for index in 0..results.len()? {
            if results.get(index)?.score()? > 1001.0 {
                count += 1;
            } else {
                break;
            }
        }
        Ok(count == expected)
    }
}

fn append_words(random: &mut FastRand, output: &mut String, count: u32) {
    for _ in 0..count {
        output.push_str(WORDS[random.next_less_than(WORDS.len() as u32) as usize]);
    }
}

pub struct Eval;

impl Case for Eval {
    type Expectation = i32;

    fn build_request(
        schema: &CompiledSchema,
        arena: &mut ExclusiveArena,
        random: &mut FastRand,
    ) -> BenchResult<Self::Expectation> {
        let mut request = expression::Builder::init_root(schema, arena)?;
        make_expression(random, &mut request, 0)
    }

    fn handle_request(
        schema: &Arc<CompiledSchema>,
        request: Arc<OwnedMessage>,
    ) -> BenchResult<ExclusiveArena> {
        let request = expression::Reader::from_root(Arc::clone(schema), request)?;
        let value = evaluate_expression(&request)?;
        let mut response = new_arena()?;
        evaluation_result::Builder::init_root(schema, &mut response)?.set_value(value)?;
        Ok(response)
    }

    fn check_response(
        schema: &Arc<CompiledSchema>,
        response: Arc<OwnedMessage>,
        expected: Self::Expectation,
    ) -> BenchResult<bool> {
        Ok(
            evaluation_result::Reader::from_root(Arc::clone(schema), response)?.value()?
                == expected,
        )
    }
}

fn make_expression(
    random: &mut FastRand,
    expression: &mut expression::Builder<'_, '_>,
    depth: u32,
) -> BenchResult<i32> {
    let operation = Operation::from_ordinal(random.next_less_than(5) as u16);
    expression.set_op(operation)?;
    let left = {
        let mut left = expression.left()?;
        if random.next_less_than(8) < depth {
            let value = (random.next_less_than(128) + 1) as i32;
            left.set_value(value)?;
            value
        } else {
            make_expression(random, &mut left.init_expression()?, depth + 1)?
        }
    };
    let right = {
        let mut right = expression.right()?;
        if random.next_less_than(8) < depth {
            let value = (random.next_less_than(128) + 1) as i32;
            right.set_value(value)?;
            value
        } else {
            make_expression(random, &mut right.init_expression()?, depth + 1)?
        }
    };
    Ok(apply(operation, left, right))
}

fn evaluate_expression(expression: &expression::Reader) -> BenchResult<i32> {
    let left = match expression.left()?.which()? {
        left::Which::Value => expression.left()?.value()?,
        left::Which::Expression => evaluate_expression(
            &expression
                .left()?
                .expression()?
                .ok_or("left expression is null")?,
        )?,
        left::Which::Unrecognized(_) => return Err("unrecognized left union member".into()),
    };
    let right = match expression.right()?.which()? {
        right::Which::Value => expression.right()?.value()?,
        right::Which::Expression => evaluate_expression(
            &expression
                .right()?
                .expression()?
                .ok_or("right expression is null")?,
        )?,
        right::Which::Unrecognized(_) => return Err("unrecognized right union member".into()),
    };
    Ok(apply(expression.op()?, left, right))
}

fn apply(operation: Operation, left: i32, right: i32) -> i32 {
    match operation {
        Operation::Add => left.wrapping_add(right),
        Operation::Subtract => left.wrapping_sub(right),
        Operation::Multiply => left.wrapping_mul(right),
        Operation::Divide => safe_divide(left, right),
        Operation::Modulus => safe_modulus(left, right),
        Operation::Unrecognized(_) => unreachable!("the benchmark only emits known operations"),
    }
}

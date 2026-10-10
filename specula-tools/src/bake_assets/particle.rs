//! Loads particle emitter types, porting `CParticleEmitterType::LoadXML`
//! (graphics/ParticleEmitterType.cpp).

use std::fs;
use std::path::Path;

use roxmltree::Node;
use specula_assets::files;
use specula_assets::files::Emitter;
use specula_assets::files::PARTICLE_VARIABLES;
use specula_assets::files::ParticleVariable;

use crate::actor::to_float;

pub struct ParticleType {
    /// A path under the mod; `None` where the engine would draw its error
    /// texture.
    pub texture: Option<String>,
    /// Its texture is left for the manifest to fill in.
    pub emitter: Emitter,
}

/// `path` is under the mod. A file that cannot be read keeps the defaults,
/// as the engine's emitter does when its XML fails to load.
pub fn load(mod_dir: &Path, path: &str) -> ParticleType {
    let mut particle = ParticleType {
        texture: None,
        emitter: Emitter {
            texture: None,
            blend: files::BLEND_ADD,
            start_full: false,
            relative_velocity: false,
            variables: defaults(),
            force: [0.0; 3],
        },
    };
    let xml = match fs::read_to_string(mod_dir.join(path)) {
        Ok(xml) => xml,
        Err(error) => {
            eprintln!("particles {path}: {error}");
            return particle;
        }
    };
    let document = match roxmltree::Document::parse(&xml) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("particles {path}: {error}");
            return particle;
        }
    };
    let emitter = &mut particle.emitter;
    for child in document.root_element().children().filter(Node::is_element) {
        let number = |name: &str| child.attribute(name).map_or(0.0, to_float);
        let id = child.attribute("name").and_then(variable_id);
        match child.tag_name().name() {
            "texture" => {
                particle.texture = Some(child.text().unwrap_or_default().trim().to_string());
            }
            "blend" => match child.attribute("mode") {
                Some("add") => emitter.blend = files::BLEND_ADD,
                Some("subtract") => emitter.blend = files::BLEND_SUBTRACT,
                Some("over") => emitter.blend = files::BLEND_OVER,
                Some("multiply") => emitter.blend = files::BLEND_MULTIPLY,
                _ => {}
            },
            "start_full" => emitter.start_full = true,
            "use_relative_velocity" => emitter.relative_velocity = true,
            "constant" => {
                if let Some(id) = id {
                    emitter.variables[id] = ParticleVariable::Constant(number("value"));
                }
            }
            "uniform" => {
                if let Some(id) = id {
                    let (min, max) = (number("min"), number("max"));
                    // The engine's own guard against hanging its RNG on an
                    // empty range.
                    emitter.variables[id] = if min < max {
                        ParticleVariable::Uniform(min, max)
                    } else {
                        ParticleVariable::Constant(min)
                    };
                }
            }
            "copy" => {
                let from = child.attribute("from").and_then(variable_id);
                if let (Some(id), Some(from)) = (id, from) {
                    emitter.variables[id] = ParticleVariable::Copy(from as u8);
                }
            }
            "expr" => {
                if let Some(id) = id {
                    emitter.variables[id] = ParticleVariable::Expr {
                        from: child.attribute("from").unwrap_or_default().to_string(),
                        mul: number("mul"),
                        max: number("max"),
                    };
                }
            }
            "force" => {
                for (sum, axis) in emitter.force.iter_mut().zip(["x", "y", "z"]) {
                    *sum += number(axis);
                }
            }
            _ => {}
        }
    }
    particle
}

/// `CParticleEmitterType::LoadXML`'s "sane defaults".
fn defaults() -> [ParticleVariable; PARTICLE_VARIABLES] {
    let mut values = [0.0; PARTICLE_VARIABLES];
    values[files::VAR_EMISSION_RATE] = 10.0;
    values[files::VAR_LIFETIME] = 3.0;
    values[files::VAR_VELOCITY_Y] = 1.0;
    values[files::VAR_SIZE] = 1.0;
    values[files::VAR_COLOR_R] = 1.0;
    values[files::VAR_COLOR_G] = 1.0;
    values[files::VAR_COLOR_B] = 1.0;
    values.map(ParticleVariable::Constant)
}

/// `CParticleEmitterType::GetVariableID`.
fn variable_id(name: &str) -> Option<usize> {
    let id = match name {
        "emissionrate" => files::VAR_EMISSION_RATE,
        "lifetime" => files::VAR_LIFETIME,
        "position.x" => files::VAR_POSITION_X,
        "position.y" => files::VAR_POSITION_Y,
        "position.z" => files::VAR_POSITION_Z,
        "angle" => files::VAR_ANGLE,
        "velocity.x" => files::VAR_VELOCITY_X,
        "velocity.y" => files::VAR_VELOCITY_Y,
        "velocity.z" => files::VAR_VELOCITY_Z,
        "velocity.angle" => files::VAR_VELOCITY_ANGLE,
        "size" => files::VAR_SIZE,
        "size.growthRate" => files::VAR_SIZE_GROWTH_RATE,
        "color.r" => files::VAR_COLOR_R,
        "color.g" => files::VAR_COLOR_G,
        "color.b" => files::VAR_COLOR_B,
        _ => {
            eprintln!("particle sets unknown variable '{name}'");
            return None;
        }
    };
    Some(id)
}

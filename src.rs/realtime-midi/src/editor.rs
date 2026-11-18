use crate::quantizer::midi_note_name;
use crate::{MtzRealtimeMidiParams, SharedUiTelemetry, HISTORY_FRAMES};
use nih_plug::prelude::{Editor, Enum, EnumParam, FloatParam, IntParam, Param, ParamSetter};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(820, 560)
}

pub fn create(
    params: Arc<MtzRealtimeMidiParams>,
    telemetry: Arc<SharedUiTelemetry>,
) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        (),
        |egui_ctx, _| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(18, 20, 26);
            visuals.window_fill = egui::Color32::from_rgb(22, 25, 32);
            egui_ctx.set_visuals(visuals);
        },
        move |egui_ctx, setter, _state| {
            egui::CentralPanel::default().show(egui_ctx, |ui| {
                draw_ui(ui, &params, setter, &telemetry);
            });
            // Keep repainting smoothly at ~30-60 fps while editor is open for live meters & piano roll
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(25));
        },
    )
}

fn draw_ui(
    ui: &mut egui::Ui,
    params: &MtzRealtimeMidiParams,
    setter: &ParamSetter,
    telemetry: &SharedUiTelemetry,
) {
    let sr = f32::from_bits(telemetry.sample_rate_bits.load(Ordering::Relaxed)).max(8000.0);
    let hop = telemetry.hop_size.load(Ordering::Relaxed);
    let win_samples = params.window_size.value().size();
    let hop_ms = (hop as f32 * 1000.0) / sr;
    let win_ms = (win_samples as f32 * 1000.0) / sr;
    let active_voices = telemetry.active_voices.load(Ordering::Relaxed);
    let total_emitted = telemetry.total_notes_emitted.load(Ordering::Relaxed);

    // --- 1. Top Header Bar ---
    ui.horizontal(|ui| {
        ui.heading(
            egui::RichText::new("MTZ Realtime MIDI")
                .color(egui::Color32::from_rgb(90, 210, 255))
                .strong(),
        );
        ui.label(
            egui::RichText::new("2-Ch FX Bus Audio \u{2192} MIDI Converter")
                .color(egui::Color32::from_gray(170)),
        );
        ui.separator();

        if ui
            .button(egui::RichText::new("\u{23F9} MIDI Panic").color(egui::Color32::from_rgb(255, 110, 110)))
            .on_hover_text("Send All Notes Off across active MIDI channels")
            .clicked()
        {
            telemetry.panic_requested.store(true, Ordering::Release);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Window: {win_ms:.1} ms | Hop: {hop_ms:.1} ms | Active Notes: {active_voices} | Emitted: {total_emitted}"
                ))
                .monospace()
                .size(11.5)
                .color(egui::Color32::from_gray(195)),
            );
        });
    });

    ui.separator();

    // --- 2. Mode, Window & Stereo FX Bus Routing Bar ---
    ui.horizontal(|ui| {
        draw_enum_combo(ui, setter, &params.detection_mode, "Mode", 205.0);
        ui.separator();
        draw_enum_combo(ui, setter, &params.stereo_routing, "FX Bus Stereo", 200.0);
        ui.separator();
        draw_enum_combo(ui, setter, &params.window_size, "Window", 170.0);
        ui.separator();
        draw_bool_toggle(ui, setter, &params.audio_thru, "Audio Pass-Thru");
    });

    ui.separator();

    // --- 3. Live Telemetry Banner: L/R Level Meter + Gate Threshold + Primary Detected Pitch ---
    let rms_db_a = f32::from_bits(telemetry.rms_db_a_bits.load(Ordering::Relaxed));
    let flux_a = f32::from_bits(telemetry.flux_a_bits.load(Ordering::Relaxed));
    let centroid_a = f32::from_bits(telemetry.centroid_a_bits.load(Ordering::Relaxed));
    let primary_note = telemetry.primary_note_a.load(Ordering::Relaxed);
    let primary_freq = f32::from_bits(telemetry.primary_freq_a_bits.load(Ordering::Relaxed));
    let primary_cents = f32::from_bits(telemetry.primary_cents_a_bits.load(Ordering::Relaxed));
    let primary_conf = f32::from_bits(telemetry.primary_conf_a_bits.load(Ordering::Relaxed));

    egui::Frame::group(ui.style())
        .fill(egui::Color32::from_rgb(14, 16, 22))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Left box: Primary pitch badge
                ui.vertical(|ui| {
                    ui.set_min_width(185.0);
                    ui.label(
                        egui::RichText::new("DETECTED PITCH")
                            .size(10.5)
                            .color(egui::Color32::from_gray(150)),
                    );
                    if primary_note < 128 {
                        let name = midi_note_name(primary_note);
                        let sign = if primary_cents >= 0.0 { "+" } else { "" };
                        ui.label(
                            egui::RichText::new(format!("{name} (#{primary_note})"))
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_rgb(80, 240, 175))
                                .monospace(),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{primary_freq:.1} Hz ({sign}{primary_cents:.0} ct) | Conf {:02.0}%",
                                primary_conf * 100.0
                            ))
                            .size(11.0)
                            .monospace()
                            .color(egui::Color32::from_gray(190)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("--")
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_gray(95))
                                .monospace(),
                        );
                        ui.label(
                            egui::RichText::new("Waiting for signal above gate...")
                                .size(11.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                    }
                });

                ui.separator();

                // Right area: Live RMS dB bar vs Onset / Release Thresholds + Flux & Brightness meters
                ui.vertical(|ui| {
                    let onset_db = params.onset_threshold_db.value();
                    let rel_db = onset_db - params.release_hysteresis_db.value().abs();

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Bus Level: {rms_db_a:5.1} dB (Onset: {onset_db:.1} dB / Release: {rel_db:.1} dB)"
                            ))
                            .monospace()
                            .size(11.5),
                        );
                    });

                    // Draw horizontal level bar (-72 dB .. 0 dB) with threshold markers
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().max(260.0), 18.0),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(28, 32, 42));

                    let db_to_norm = |db: f32| ((db + 72.0) / 72.0).clamp(0.0, 1.0);
                    let level_norm = db_to_norm(rms_db_a);
                    let onset_norm = db_to_norm(onset_db);
                    let rel_norm = db_to_norm(rel_db);

                    let bar_color = if rms_db_a >= onset_db {
                        egui::Color32::from_rgb(65, 220, 140)
                    } else if rms_db_a >= rel_db {
                        egui::Color32::from_rgb(230, 185, 65)
                    } else {
                        egui::Color32::from_rgb(70, 120, 180)
                    };

                    let mut fill_rect = rect;
                    fill_rect.max.x = rect.min.x + rect.width() * level_norm;
                    painter.rect_filled(fill_rect, 3.0, bar_color);

                    // Release marker line (yellow)
                    let rel_x = rect.min.x + rect.width() * rel_norm;
                    painter.line_segment(
                        [egui::pos2(rel_x, rect.min.y), egui::pos2(rel_x, rect.max.y)],
                        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(240, 190, 60)),
                    );

                    // Onset marker line (red/orange)
                    let onset_x = rect.min.x + rect.width() * onset_norm;
                    painter.line_segment(
                        [
                            egui::pos2(onset_x, rect.min.y),
                            egui::pos2(onset_x, rect.max.y),
                        ],
                        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 95, 95)),
                    );

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Transient Flux: {:3.0}%", flux_a * 100.0))
                                .monospace()
                                .size(11.0),
                        );
                        ui.add(
                            egui::ProgressBar::new(flux_a.clamp(0.0, 1.0))
                                .desired_width(140.0),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!(
                                "Spectral Brightness: {:3.0}%",
                                centroid_a * 100.0
                            ))
                            .monospace()
                            .size(11.0),
                        );
                        ui.add(
                            egui::ProgressBar::new(centroid_a.clamp(0.0, 1.0))
                                .desired_width(140.0),
                        );
                    });
                });
            });
        });

    ui.add_space(4.0);

    // --- 4. Live Rolling Pitch Trail & 88-Key Active MIDI Piano Roll ---
    let kb_snap = telemetry.keyboard.lock().clone();
    draw_piano_and_history(
        ui,
        &kb_snap,
        params.min_note.value() as u8,
        params.max_note.value() as u8,
    );

    ui.add_space(6.0);

    // --- 5. Four-Column Parameter Controls ---
    ui.columns(4, |cols| {
        // Column 0: Gate & Detection
        egui::Frame::group(cols[0].style()).show(&mut cols[0], |ui| {
            ui.label(
                egui::RichText::new("1. Gate & Detection")
                    .strong()
                    .color(egui::Color32::from_rgb(110, 200, 255)),
            );
            ui.separator();
            draw_float_slider(ui, setter, &params.input_gain_db, "Input Trim");
            draw_float_slider(ui, setter, &params.onset_threshold_db, "Onset Gate");
            draw_float_slider(ui, setter, &params.release_hysteresis_db, "Hysteresis");
            draw_float_slider(ui, setter, &params.min_confidence, "Pitch Clarity");
        });

        // Column 1: Note Tracking & Polyphony
        egui::Frame::group(cols[1].style()).show(&mut cols[1], |ui| {
            ui.label(
                egui::RichText::new("2. Note Tracking")
                    .strong()
                    .color(egui::Color32::from_rgb(110, 200, 255)),
            );
            ui.separator();
            draw_int_slider(ui, setter, &params.max_polyphony, "Max Polyphony");
            draw_float_slider(ui, setter, &params.min_hold_ms, "Min Hold (ms)");
            draw_int_slider(ui, setter, &params.stability_hops, "Stability Hops");
            draw_float_slider(ui, setter, &params.retrigger_flux, "Re-Trigger Flux");
        });

        // Column 2: Pitch, Scale & Range
        egui::Frame::group(cols[2].style()).show(&mut cols[2], |ui| {
            ui.label(
                egui::RichText::new("3. Scale & Range")
                    .strong()
                    .color(egui::Color32::from_rgb(110, 200, 255)),
            );
            ui.separator();
            draw_enum_combo(ui, setter, &params.root_key, "Root", 110.0);
            draw_enum_combo(ui, setter, &params.scale_type, "Scale", 130.0);
            draw_int_slider(ui, setter, &params.transpose_semitones, "Transpose");
            draw_int_slider(ui, setter, &params.min_note, "Min Note");
            draw_int_slider(ui, setter, &params.max_note, "Max Note");
            draw_float_slider(ui, setter, &params.a4_tuning_hz, "A4 Ref (Hz)");
        });

        // Column 3: MIDI Output & Expression
        egui::Frame::group(cols[3].style()).show(&mut cols[3], |ui| {
            ui.label(
                egui::RichText::new("4. MIDI & Expression")
                    .strong()
                    .color(egui::Color32::from_rgb(110, 200, 255)),
            );
            ui.separator();
            draw_int_slider(ui, setter, &params.midi_channel, "Base Channel");
            draw_float_slider(ui, setter, &params.velocity_sensitivity, "Vel Sensitivity");
            draw_int_slider(ui, setter, &params.min_velocity, "Min Velocity");
            draw_bool_toggle(ui, setter, &params.pitch_bend_enabled, "Mono Pitch Bend");
            draw_float_slider(ui, setter, &params.pitch_bend_range, "PB Range (st)");
            draw_enum_combo(ui, setter, &params.env_cc_target, "Env CC", 125.0);
            draw_bool_toggle(ui, setter, &params.brightness_cc74, "Centroid \u{2192} CC74");
        });
    });
}

fn draw_piano_and_history(
    ui: &mut egui::Ui,
    kb: &crate::KeyboardSnapshot,
    min_note: u8,
    max_note: u8,
) {
    let total_width = ui.available_width().max(300.0);

    // 1. Rolling Pitch History Strip (64 px tall)
    let (hist_rect, _) =
        ui.allocate_exact_size(egui::vec2(total_width, 64.0), egui::Sense::hover());
    let painter = ui.painter_at(hist_rect);
    painter.rect_filled(hist_rect, 4.0, egui::Color32::from_rgb(12, 14, 19));

    let note_lo = 21.0_f32;
    let note_hi = 108.0_f32;
    let col_w = hist_rect.width() / (HISTORY_FRAMES as f32);

    for i in 0..HISTORY_FRAMES {
        let idx = (kb.history_head + i) % HISTORY_FRAMES;
        let col = kb.history[idx];
        let x = hist_rect.min.x + (i as f32) * col_w;

        if col.primary_note_a <= 108 && col.primary_note_a >= 21 {
            let norm = (col.primary_note_a as f32 - note_lo) / (note_hi - note_lo);
            let y = hist_rect.max.y - 4.0 - norm * (hist_rect.height() - 8.0);
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y - 2.0), egui::vec2(col_w.max(2.0), 4.0)),
                1.0,
                egui::Color32::from_rgb(70, 235, 165),
            );
        }
        if col.primary_note_b <= 108 && col.primary_note_b >= 21 {
            let norm = (col.primary_note_b as f32 - note_lo) / (note_hi - note_lo);
            let y = hist_rect.max.y - 4.0 - norm * (hist_rect.height() - 8.0);
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y - 2.0), egui::vec2(col_w.max(2.0), 4.0)),
                1.0,
                egui::Color32::from_rgb(255, 165, 70),
            );
        }
    }

    ui.add_space(2.0);

    // 2. 88-Key Piano Strip (MIDI notes 21..=108, 42 px tall)
    let (kb_rect, _) = ui.allocate_exact_size(egui::vec2(total_width, 42.0), egui::Sense::hover());
    let kb_painter = ui.painter_at(kb_rect);
    kb_painter.rect_filled(kb_rect, 2.0, egui::Color32::from_rgb(20, 22, 28));

    let num_keys = (108 - 21 + 1) as usize;
    let key_w = kb_rect.width() / (num_keys as f32);
    let lo_bound = min_note.min(max_note);
    let hi_bound = min_note.max(max_note);

    for note in 21u8..=108u8 {
        let i = (note - 21) as usize;
        let x0 = kb_rect.min.x + (i as f32) * key_w;
        let k_rect =
            egui::Rect::from_min_size(egui::pos2(x0, kb_rect.min.y), egui::vec2(key_w - 0.8, 42.0));

        let pc = note % 12;
        let is_black = matches!(pc, 1 | 3 | 6 | 8 | 10);
        let in_range = note >= lo_bound && note <= hi_bound;

        let vel_a = kb.active_a[note as usize];
        let vel_b = kb.active_b[note as usize];

        let color = if vel_a > 0 {
            egui::Color32::from_rgb(50, 235, 160)
        } else if vel_b > 0 {
            egui::Color32::from_rgb(255, 160, 60)
        } else if !in_range {
            egui::Color32::from_rgb(26, 28, 34)
        } else if is_black {
            egui::Color32::from_rgb(42, 46, 58)
        } else {
            egui::Color32::from_rgb(175, 182, 195)
        };

        kb_painter.rect_filled(k_rect, 1.5, color);
    }
}

fn draw_float_slider(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &FloatParam,
    label: &str,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}:")).size(11.5));
        let mut norm = param.unmodulated_normalized_value();
        let display = param.to_string();
        let slider = egui::Slider::from_get_set(0.0..=1.0, |new_val| {
            if let Some(v) = new_val {
                norm = v as f32;
            }
            norm as f64
        })
        .text(display)
        .show_value(false);

        let resp = ui.add(slider);
        if resp.drag_started() {
            setter.begin_set_parameter(param);
        }
        if resp.changed() {
            setter.set_parameter_normalized(param, norm);
        }
        if resp.drag_stopped() {
            setter.end_set_parameter(param);
        }
        if resp.double_clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, param.default_normalized_value());
            setter.end_set_parameter(param);
        }
    });
}

fn draw_int_slider(ui: &mut egui::Ui, setter: &ParamSetter, param: &IntParam, label: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}:")).size(11.5));
        let mut norm = param.unmodulated_normalized_value();
        let display = param.to_string();
        let slider = egui::Slider::from_get_set(0.0..=1.0, |new_val| {
            if let Some(v) = new_val {
                norm = v as f32;
            }
            norm as f64
        })
        .text(display)
        .show_value(false);

        let resp = ui.add(slider);
        if resp.drag_started() {
            setter.begin_set_parameter(param);
        }
        if resp.changed() {
            setter.set_parameter_normalized(param, norm);
        }
        if resp.drag_stopped() {
            setter.end_set_parameter(param);
        }
        if resp.double_clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, param.default_normalized_value());
            setter.end_set_parameter(param);
        }
    });
}

fn draw_bool_toggle(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &nih_plug::prelude::BoolParam,
    label: &str,
) {
    let mut val = param.value();
    if ui.checkbox(&mut val, label).changed() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, val);
        setter.end_set_parameter(param);
    }
}

fn draw_enum_combo<T: Enum + PartialEq>(
    ui: &mut egui::Ui,
    setter: &ParamSetter,
    param: &EnumParam<T>,
    label: &str,
    width: f32,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{label}:")).size(11.5));
        let current_idx = param.unmodulated_plain_value().to_index();
        let descriptors = T::variants();
        let current_name = descriptors.get(current_idx).copied().unwrap_or("Unknown");

        egui::ComboBox::from_id_salt(label)
            .width(width)
            .selected_text(current_name)
            .show_ui(ui, |ui| {
                for (idx, &variant_name) in descriptors.iter().enumerate() {
                    if ui.selectable_label(idx == current_idx, variant_name).clicked() {
                        setter.begin_set_parameter(param);
                        setter.set_parameter(param, T::from_index(idx));
                        setter.end_set_parameter(param);
                    }
                }
            });
    });
}

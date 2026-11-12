//! UVC controls (camera terminal, processing unit, extension units...).
//!
//! A [`Control`] stores its values as raw little-endian bytes (that is what
//! goes over the wire) plus a [`Validation`] rule for SET_CUR. Constructors
//! cover the usual shapes; [`standard`] gives spec-compliant defaults for
//! every standard selector, which device models then adjust.

use crate::consts::*;

/// How SET_CUR values are checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Validation {
    /// Anything of the right length.
    None,
    /// Little-endian integer fields, each within [min, max] and on the `res` grid.
    Fields(Vec<Field>),
    /// Exactly one bit set, within the supported mask (e.g. CT_AE_MODE).
    OneBitOf(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub size: u8,
    pub signed: bool,
    pub min: i64,
    pub max: i64,
    pub res: i64,
    pub def: i64,
}

impl Field {
    pub const fn new(size: u8, signed: bool, min: i64, max: i64, res: i64, def: i64) -> Self {
        Field {
            size,
            signed,
            min,
            max,
            res,
            def,
        }
    }
}

fn put(out: &mut Vec<u8>, size: u8, v: i64) {
    out.extend_from_slice(&v.to_le_bytes()[..size as usize]);
}

fn get(b: &[u8], size: u8, signed: bool) -> i64 {
    let mut raw = [0u8; 8];
    raw[..size as usize].copy_from_slice(&b[..size as usize]);
    let v = i64::from_le_bytes(raw);
    if signed && size < 8 {
        let shift = 64 - 8 * size as u32;
        (v << shift) >> shift
    } else {
        v
    }
}

/// One control and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    /// GET_LEN.
    pub len: u16,
    /// GET_INFO capability bits (D0 GET, D1 SET, D3 autoupdate, D4 async).
    pub info: u8,
    /// None = request unsupported (STALL, "invalid request").
    pub min: Option<Vec<u8>>,
    pub max: Option<Vec<u8>>,
    pub res: Option<Vec<u8>>,
    pub def: Option<Vec<u8>>,
    pub cur: Vec<u8>,
    pub validation: Validation,
    /// Disabled because an associated auto control is on (GET_INFO D2):
    /// SET_CUR fails with "wrong state".
    pub disabled: bool,
}

impl Control {
    /// Integer fields with full range support (GET_MIN/MAX/RES/DEF).
    pub fn fields(fields: &[Field]) -> Self {
        let (mut min, mut max, mut res, mut def) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for f in fields {
            put(&mut min, f.size, f.min);
            put(&mut max, f.size, f.max);
            put(&mut res, f.size, f.res);
            put(&mut def, f.size, f.def);
        }
        Control {
            len: def.len() as u16,
            info: INFO_GET | INFO_SET,
            min: Some(min),
            max: Some(max),
            res: Some(res),
            cur: def.clone(),
            def: Some(def),
            validation: Validation::Fields(fields.to_vec()),
            disabled: false,
        }
    }

    /// Single integer with full range support.
    pub fn int(size: u8, signed: bool, min: i64, max: i64, res: i64, def: i64) -> Self {
        Self::fields(&[Field::new(size, signed, min, max, res, def)])
    }

    /// One-byte on/off (or small enum) control: GET_CUR/SET_CUR/GET_DEF only.
    pub fn switch(def: u8, max: u8) -> Self {
        let mut c = Self::int(1, false, 0, max as i64, 1, def as i64);
        c.min = None;
        c.max = None;
        c.res = None;
        c
    }

    /// Bitmap of modes: GET_RES = supported modes, SET_CUR must pick exactly one.
    pub fn one_bit_of(size: u8, supported: u64, def: u64) -> Self {
        let mut res = Vec::new();
        let mut d = Vec::new();
        put(&mut res, size, supported as i64);
        put(&mut d, size, def as i64);
        Control {
            len: size as u16,
            info: INFO_GET | INFO_SET,
            min: None,
            max: None,
            res: Some(res),
            cur: d.clone(),
            def: Some(d),
            validation: Validation::OneBitOf(supported),
            disabled: false,
        }
    }

    /// Opaque bytes (extension unit controls): every request supported, any value accepted.
    pub fn raw(len: u16) -> Self {
        let z = vec![0u8; len as usize];
        Control {
            len,
            info: INFO_GET | INFO_SET,
            min: Some(z.clone()),
            max: Some(vec![0xff; len as usize]),
            res: Some(z.clone()),
            def: Some(z.clone()),
            cur: z,
            validation: Validation::None,
            disabled: false,
        }
    }

    pub fn with_info(mut self, info: u8) -> Self {
        self.info = info;
        self
    }

    /// Restricts the requests supported besides GET_CUR/SET_CUR/GET_INFO/GET_LEN.
    pub fn only(mut self, min: bool, max: bool, res: bool, def: bool) -> Self {
        if !min {
            self.min = None;
        }
        if !max {
            self.max = None;
        }
        if !res {
            self.res = None;
        }
        if !def {
            self.def = None;
        }
        self
    }

    /// Current value of field `i`, for fields-validated controls.
    pub fn field(&self, i: usize) -> Option<i64> {
        let Validation::Fields(fs) = &self.validation else {
            return (i == 0 && !self.cur.is_empty())
                .then(|| get(&self.cur, self.cur.len().min(8) as u8, false));
        };
        let off: usize = fs[..i].iter().map(|f| f.size as usize).sum();
        let f = fs.get(i)?;
        Some(get(&self.cur[off..], f.size, f.signed))
    }

    /// Current value as an unsigned little-endian integer (up to 8 bytes).
    pub fn value(&self) -> u64 {
        get(&self.cur, self.cur.len().min(8) as u8, false) as u64
    }

    /// Answers a GET request. Errors are UVC request error codes.
    pub fn get(&self, request: u8) -> Result<Vec<u8>, u8> {
        let v = match request {
            GET_CUR if self.info & INFO_GET != 0 => Some(self.cur.clone()),
            GET_CUR => None,
            GET_MIN => self.min.clone(),
            GET_MAX => self.max.clone(),
            GET_RES => self.res.clone(),
            GET_DEF => self.def.clone(),
            GET_LEN => Some(self.len.to_le_bytes().to_vec()),
            GET_INFO => Some(vec![
                self.info | if self.disabled { INFO_DISABLED } else { 0 },
            ]),
            _ => None,
        };
        v.ok_or(ERR_INVALID_REQUEST)
    }

    /// Checks a SET_CUR value without applying it.
    pub fn check(&self, data: &[u8]) -> Result<(), u8> {
        if self.info & INFO_SET == 0 {
            return Err(ERR_INVALID_REQUEST);
        }
        if self.disabled {
            return Err(ERR_WRONG_STATE);
        }
        if data.len() != self.len as usize {
            return Err(ERR_UNKNOWN);
        }
        match &self.validation {
            Validation::None => Ok(()),
            Validation::OneBitOf(mask) => {
                let v = get(data, data.len().min(8) as u8, false) as u64;
                if v.count_ones() == 1 && v & mask != 0 {
                    Ok(())
                } else {
                    Err(ERR_OUT_OF_RANGE)
                }
            }
            Validation::Fields(fs) => {
                let mut off = 0;
                for f in fs {
                    let v = get(&data[off..], f.size, f.signed);
                    off += f.size as usize;
                    if v < f.min || v > f.max {
                        return Err(ERR_OUT_OF_RANGE);
                    }
                    if f.res > 1 && (v - f.min) % f.res != 0 {
                        return Err(ERR_INVALID_VALUE_WITHIN_RANGE);
                    }
                }
                Ok(())
            }
        }
    }

    pub fn set(&mut self, data: &[u8]) -> Result<(), u8> {
        self.check(data)?;
        self.cur = data.to_vec();
        Ok(())
    }

    /// Back to the default value (bus reset).
    pub fn reset(&mut self) {
        if let Some(d) = &self.def {
            self.cur = d.clone();
        }
    }
}

/// Which kind of entity a selector belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Camera,
    Processing,
}

/// Spec-compliant default for a standard control (sizes and supported
/// requests follow UVC 1.5 / the Linux uvcvideo control table; ranges are
/// plausible webcam values). `None` for selectors without a sensible default.
pub fn standard(unit: Unit, selector: u8) -> Option<Control> {
    const AUTO: u8 = INFO_GET | INFO_SET | INFO_AUTOUPDATE;
    Some(match (unit, selector) {
        (Unit::Camera, CT_SCANNING_MODE_CONTROL) => {
            Control::switch(1, 1).only(false, false, false, false)
        }
        (Unit::Camera, CT_AE_MODE_CONTROL) => Control::one_bit_of(1, 0x09, 0x08),
        (Unit::Camera, CT_AE_PRIORITY_CONTROL) => {
            Control::switch(0, 1).only(false, false, false, false)
        }
        (Unit::Camera, CT_EXPOSURE_TIME_ABSOLUTE_CONTROL) => {
            Control::int(4, false, 3, 2047, 1, 250).with_info(AUTO)
        }
        (Unit::Camera, CT_EXPOSURE_TIME_RELATIVE_CONTROL) => Control::int(1, true, -1, 1, 1, 0)
            .only(false, false, false, false)
            .with_info(INFO_SET),
        (Unit::Camera, CT_FOCUS_ABSOLUTE_CONTROL) => {
            Control::int(2, false, 0, 250, 5, 0).with_info(AUTO)
        }
        (Unit::Camera, CT_FOCUS_RELATIVE_CONTROL) => Control::fields(&[
            Field::new(1, true, -1, 1, 1, 0),
            Field::new(1, false, 1, 1, 1, 1),
        ])
        .with_info(AUTO),
        (Unit::Camera, CT_FOCUS_AUTO_CONTROL) => Control::switch(1, 1),
        (Unit::Camera, CT_IRIS_ABSOLUTE_CONTROL) => {
            Control::int(2, false, 0, 4000, 100, 2000).with_info(AUTO)
        }
        (Unit::Camera, CT_IRIS_RELATIVE_CONTROL) => Control::int(1, true, -1, 1, 1, 0)
            .only(false, false, false, false)
            .with_info(INFO_SET),
        (Unit::Camera, CT_ZOOM_ABSOLUTE_CONTROL) => {
            Control::int(2, false, 0, 10, 1, 0).with_info(AUTO)
        }
        (Unit::Camera, CT_ZOOM_RELATIVE_CONTROL) => Control::fields(&[
            Field::new(1, true, -1, 1, 1, 0),
            Field::new(1, false, 0, 1, 1, 0),
            Field::new(1, false, 1, 1, 1, 1),
        ])
        .with_info(AUTO),
        (Unit::Camera, CT_PANTILT_ABSOLUTE_CONTROL) => Control::fields(&[
            Field::new(4, true, -36000, 36000, 3600, 0),
            Field::new(4, true, -36000, 36000, 3600, 0),
        ])
        .with_info(AUTO),
        (Unit::Camera, CT_PANTILT_RELATIVE_CONTROL) => Control::fields(&[
            Field::new(1, true, -1, 1, 1, 0),
            Field::new(1, false, 1, 1, 1, 1),
            Field::new(1, true, -1, 1, 1, 0),
            Field::new(1, false, 1, 1, 1, 1),
        ])
        .with_info(AUTO),
        (Unit::Camera, CT_ROLL_ABSOLUTE_CONTROL) => {
            Control::int(2, true, -180, 180, 1, 0).with_info(AUTO)
        }
        (Unit::Camera, CT_ROLL_RELATIVE_CONTROL) => Control::fields(&[
            Field::new(1, true, -1, 1, 1, 0),
            Field::new(1, false, 1, 1, 1, 1),
        ])
        .with_info(AUTO),
        (Unit::Camera, CT_PRIVACY_CONTROL) => Control::switch(0, 1)
            .only(false, false, false, false)
            .with_info(AUTO),
        (Unit::Camera, CT_FOCUS_SIMPLE_CONTROL) => Control::int(1, false, 0, 3, 1, 0),
        (Unit::Camera, CT_REGION_OF_INTEREST_CONTROL) => region_of_interest(640, 480),
        (Unit::Processing, PU_BACKLIGHT_COMPENSATION_CONTROL) => Control::int(2, false, 0, 2, 1, 1),
        (Unit::Processing, PU_BRIGHTNESS_CONTROL) => Control::int(2, true, -64, 64, 1, 0),
        (Unit::Processing, PU_CONTRAST_CONTROL) => Control::int(2, false, 0, 64, 1, 32),
        (Unit::Processing, PU_GAIN_CONTROL) => Control::int(2, false, 0, 100, 1, 0),
        (Unit::Processing, PU_POWER_LINE_FREQUENCY_CONTROL) => {
            Control::int(1, false, 0, 2, 1, 1).only(true, true, false, true)
        }
        (Unit::Processing, PU_HUE_CONTROL) => Control::int(2, true, -40, 40, 1, 0).with_info(AUTO),
        (Unit::Processing, PU_SATURATION_CONTROL) => Control::int(2, false, 0, 128, 1, 64),
        (Unit::Processing, PU_SHARPNESS_CONTROL) => Control::int(2, false, 0, 6, 1, 3),
        (Unit::Processing, PU_GAMMA_CONTROL) => Control::int(2, false, 72, 500, 1, 100),
        (Unit::Processing, PU_WHITE_BALANCE_TEMPERATURE_CONTROL) => {
            Control::int(2, false, 2800, 6500, 1, 4600).with_info(AUTO)
        }
        (Unit::Processing, PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL) => Control::switch(1, 1),
        (Unit::Processing, PU_WHITE_BALANCE_COMPONENT_CONTROL) => Control::fields(&[
            Field::new(2, false, 0, 255, 1, 128),
            Field::new(2, false, 0, 255, 1, 128),
        ])
        .with_info(AUTO),
        (Unit::Processing, PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL) => Control::switch(1, 1),
        (Unit::Processing, PU_DIGITAL_MULTIPLIER_CONTROL) => Control::int(2, false, 1, 16, 1, 1),
        (Unit::Processing, PU_DIGITAL_MULTIPLIER_LIMIT_CONTROL) => {
            Control::int(2, false, 1, 16, 1, 16)
        }
        (Unit::Processing, PU_HUE_AUTO_CONTROL) => Control::switch(0, 1),
        (Unit::Processing, PU_ANALOG_VIDEO_STANDARD_CONTROL) => Control::switch(0, 0)
            .with_info(INFO_GET)
            .only(false, false, false, false),
        (Unit::Processing, PU_ANALOG_LOCK_STATUS_CONTROL) => Control::switch(0, 0)
            .with_info(INFO_GET)
            .only(false, false, false, false),
        (Unit::Processing, PU_CONTRAST_AUTO_CONTROL) => Control::switch(0, 1),
        _ => return None,
    })
}

/// CT_REGION_OF_INTEREST for a sensor of the given size: top, left, bottom,
/// right, bmAutoControls.
pub fn region_of_interest(width: u16, height: u16) -> Control {
    let (w, h) = (width as i64 - 1, height as i64 - 1);
    let mut c = Control::fields(&[
        Field::new(2, false, 0, h, 1, 0),
        Field::new(2, false, 0, w, 1, 0),
        Field::new(2, false, 0, h, 1, h),
        Field::new(2, false, 0, w, 1, w),
        Field::new(2, false, 0, 0xff, 1, 1),
    ])
    .with_info(INFO_GET | INFO_SET | INFO_AUTOUPDATE | INFO_ASYNC);
    // MIN/MAX describe the smallest/largest rectangles, not per-field limits.
    c.min = Some(
        [0u16, 0, 0, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    );
    c.max = Some(
        [h as u16, w as u16, h as u16, w as u16, 0xff]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    );
    c
}

/// `(auto selector, predicate on its value, dependent selectors)`.
pub type AutoLink = (u8, fn(u64) -> bool, &'static [u8]);

/// "Auto" controls and the controls they disable (same entity).
pub fn auto_links(unit: Unit) -> &'static [AutoLink] {
    fn on(v: u64) -> bool {
        v != 0
    }
    // AE modes: 1 manual, 2 auto, 4 shutter priority, 8 aperture priority.
    fn exposure_auto(v: u64) -> bool {
        v & 0x0a != 0
    }
    fn iris_auto(v: u64) -> bool {
        v & 0x06 != 0
    }
    match unit {
        Unit::Camera => &[
            (
                CT_AE_MODE_CONTROL,
                exposure_auto,
                &[
                    CT_EXPOSURE_TIME_ABSOLUTE_CONTROL,
                    CT_EXPOSURE_TIME_RELATIVE_CONTROL,
                ],
            ),
            (
                CT_AE_MODE_CONTROL,
                iris_auto,
                &[CT_IRIS_ABSOLUTE_CONTROL, CT_IRIS_RELATIVE_CONTROL],
            ),
            (
                CT_FOCUS_AUTO_CONTROL,
                on,
                &[CT_FOCUS_ABSOLUTE_CONTROL, CT_FOCUS_RELATIVE_CONTROL],
            ),
        ],
        Unit::Processing => &[
            (
                PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL,
                on,
                &[PU_WHITE_BALANCE_TEMPERATURE_CONTROL],
            ),
            (
                PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL,
                on,
                &[PU_WHITE_BALANCE_COMPONENT_CONTROL],
            ),
            (PU_HUE_AUTO_CONTROL, on, &[PU_HUE_CONTROL]),
            (PU_CONTRAST_AUTO_CONTROL, on, &[PU_CONTRAST_CONTROL]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_control() {
        let mut c = Control::int(2, true, -64, 64, 1, 0);
        assert_eq!(c.get(GET_MIN), Ok(vec![0xc0, 0xff]));
        assert_eq!(c.get(GET_LEN), Ok(vec![2, 0]));
        assert_eq!(c.set(&65i16.to_le_bytes()), Err(ERR_OUT_OF_RANGE));
        assert_eq!(c.set(&[1]), Err(ERR_UNKNOWN));
        c.set(&(-5i16).to_le_bytes()).unwrap();
        assert_eq!(c.field(0), Some(-5));
        c.disabled = true;
        assert_eq!(
            c.get(GET_INFO),
            Ok(vec![INFO_GET | INFO_SET | INFO_DISABLED])
        );
        assert_eq!(c.set(&[0, 0]), Err(ERR_WRONG_STATE));
    }

    #[test]
    fn ae_mode() {
        let mut c = standard(Unit::Camera, CT_AE_MODE_CONTROL).unwrap();
        assert_eq!(c.get(GET_MIN), Err(ERR_INVALID_REQUEST));
        assert_eq!(c.get(GET_RES), Ok(vec![0x09]));
        assert_eq!(c.set(&[0x02]), Err(ERR_OUT_OF_RANGE));
        assert_eq!(c.set(&[0x09]), Err(ERR_OUT_OF_RANGE));
        c.set(&[0x01]).unwrap();
    }

    #[test]
    fn resolution_grid() {
        let mut c = Control::int(2, false, 0, 250, 5, 0);
        assert_eq!(
            c.set(&7u16.to_le_bytes()),
            Err(ERR_INVALID_VALUE_WITHIN_RANGE)
        );
        c.set(&10u16.to_le_bytes()).unwrap();
    }
}

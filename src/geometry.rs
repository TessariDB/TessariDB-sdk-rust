//! Shapes on a sphere.
//!
//! The set is RFC 7946's — point, line, polygon, the multi- forms, and a
//! collection — so a shape leaving this client needs no translation to reach a
//! mapping tool, and one arriving needs no dialect.
//!
//! # Longitude first, and it is worth one paragraph
//!
//! RFC 7946 §3.1.1 fixes the order as `[longitude, latitude]`, and the protocol
//! writes it that way. Swapping the pair is the most common bug in geospatial
//! code and the least visible one: a point in Paris becomes a point in the
//! Indian Ocean, which is a perfectly valid place, so nothing fails. The
//! constructor therefore takes the arguments in the order the bytes are in, and
//! the fields are named rather than positional.
//!
//! # This client does not judge a shape
//!
//! A ring that does not close, a coordinate off the sphere, a line with one
//! position — all of them decode. [`Geometry::is_well_formed`] is offered for a
//! caller that wants to check before sending, and is deliberately not applied by
//! the codec: dialects of "valid" differ, and a client that refuses locally what
//! the node would accept has invented a second rule that no server enforces.

/// One position on the sphere: **longitude first**, then latitude, in degrees.
///
/// Equality is on **bits**, not on the float comparison. That is not fussiness:
/// the node compares coordinates the same way, and a client using `==` would
/// otherwise call `-0.0` and `0.0` the same coordinate while the store calls
/// them different — so a caller checking a value it just read against the one it
/// wrote could get a different answer from each side. The cost is stated here
/// rather than discovered: a NaN coordinate equals itself.
#[derive(Debug, Clone, Copy)]
pub struct Position {
    /// Degrees east of the prime meridian, in `[-180, 180]`.
    pub longitude: f64,
    /// Degrees north of the equator, in `[-90, 90]`.
    pub latitude: f64,
}

impl Position {
    /// A position, longitude first — the order the bytes are in.
    #[must_use]
    pub const fn new(longitude: f64, latitude: f64) -> Self {
        Self {
            longitude,
            latitude,
        }
    }

    /// Whether this position is on the sphere at all.
    #[must_use]
    pub fn is_on_the_sphere(&self) -> bool {
        (-180.0..=180.0).contains(&self.longitude)
            && (-90.0..=90.0).contains(&self.latitude)
            && self.longitude.is_finite()
            && self.latitude.is_finite()
    }

    const fn bits(&self) -> (u64, u64) {
        (self.longitude.to_bits(), self.latitude.to_bits())
    }
}

impl PartialEq for Position {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

/// A closed ring of positions, as a polygon's boundary.
///
/// RFC 7946 has the first and last position be the same one. Not enforced here —
/// see [`Ring::is_closed`], which an acceptance check calls.
#[derive(Debug, Clone, PartialEq)]
pub struct Ring(pub Vec<Position>);

impl Ring {
    /// Whether the ring closes, which a polygon boundary must.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        match (self.0.first(), self.0.last()) {
            // Four positions is the minimum that bounds any area: three corners
            // and the repeat that closes it.
            (Some(first), Some(last)) => self.0.len() >= 4 && first == last,
            _ => false,
        }
    }
}

/// A polygon: an outer ring, then any number of holes.
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    /// The boundary.
    pub exterior: Ring,
    /// Rings cut out of it.
    pub interiors: Vec<Ring>,
}

/// A shape.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Geometry {
    /// One position.
    Point(Position),
    /// An open path through positions.
    Line(Vec<Position>),
    /// A bounded area, possibly with holes.
    Polygon(Polygon),
    /// Several points as one shape.
    MultiPoint(Vec<Position>),
    /// Several paths as one shape.
    MultiLine(Vec<Vec<Position>>),
    /// Several areas as one shape.
    MultiPolygon(Vec<Polygon>),
    /// Shapes of mixed kinds, as one.
    ///
    /// Boxed because a collection holds geometries and a geometry may be a
    /// collection; without the box the type would have no finite size.
    Collection(Vec<Box<Geometry>>),
}

impl Geometry {
    /// The name this shape carries in a message.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Point(_) => "point",
            Self::Line(_) => "line",
            Self::Polygon(_) => "polygon",
            Self::MultiPoint(_) => "multipoint",
            Self::MultiLine(_) => "multiline",
            Self::MultiPolygon(_) => "multipolygon",
            Self::Collection(_) => "collection",
        }
    }

    /// Every position this shape is made of, in the order the bytes carry them.
    #[must_use]
    pub fn positions(&self) -> Vec<Position> {
        let mut out = Vec::new();
        self.collect_positions(&mut out);
        out
    }

    fn collect_positions(&self, into: &mut Vec<Position>) {
        match self {
            Self::Point(position) => into.push(*position),
            Self::Line(positions) | Self::MultiPoint(positions) => {
                into.extend_from_slice(positions);
            }
            Self::Polygon(polygon) => collect_polygon(polygon, into),
            Self::MultiLine(lines) => {
                for line in lines {
                    into.extend_from_slice(line);
                }
            }
            Self::MultiPolygon(polygons) => {
                for polygon in polygons {
                    collect_polygon(polygon, into);
                }
            }
            Self::Collection(shapes) => {
                for shape in shapes {
                    shape.collect_positions(into);
                }
            }
        }
    }

    /// Whether every position is on the sphere and every ring closes.
    ///
    /// Offered to a caller, never applied by the codec — see this module's
    /// header for why.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        if !self.positions().iter().all(Position::is_on_the_sphere) {
            return false;
        }
        match self {
            Self::Polygon(polygon) => polygon_is_closed(polygon),
            Self::MultiPolygon(polygons) => polygons.iter().all(polygon_is_closed),
            Self::Collection(shapes) => shapes.iter().all(|shape| shape.is_well_formed()),
            Self::Point(_) | Self::MultiPoint(_) => true,
            // A line needs two distinct ends to be a path at all.
            Self::Line(positions) => positions.len() >= 2,
            Self::MultiLine(lines) => lines.iter().all(|line| line.len() >= 2),
        }
    }
}

fn collect_polygon(polygon: &Polygon, into: &mut Vec<Position>) {
    into.extend_from_slice(&polygon.exterior.0);
    for interior in &polygon.interiors {
        into.extend_from_slice(&interior.0);
    }
}

fn polygon_is_closed(polygon: &Polygon) -> bool {
    polygon.exterior.is_closed() && polygon.interiors.iter().all(Ring::is_closed)
}

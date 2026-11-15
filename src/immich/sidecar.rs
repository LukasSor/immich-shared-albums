/** immich/sidecar.rs — the XMP sidecar a stub travels with, so Immich reads its metadata in the upload's own job. See ARCHITECTURE.md. */
use crate::immich::client::composed_description;
use crate::immich::refs::AssetRef;

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn coordinate(value: f64, positive: char, negative: char) -> String {
    let hemisphere = if value < 0.0 { negative } else { positive };
    let magnitude = value.abs();
    let degrees = magnitude.floor();
    let minutes = (magnitude - degrees) * 60.0;
    format!("{degrees:.0},{minutes:.6}{hemisphere}")
}

pub fn xmp_sidecar(reference: &AssetRef) -> Option<String> {
    let mut attributes = Vec::new();
    if let Some(exif) = &reference.exif {
        if let (Some(latitude), Some(longitude)) = (exif.latitude, exif.longitude) {
            attributes.push(format!(
                "exif:GPSLatitude=\"{}\"",
                coordinate(latitude, 'N', 'S')
            ));
            attributes.push(format!(
                "exif:GPSLongitude=\"{}\"",
                coordinate(longitude, 'E', 'W')
            ));
        }
        if let Some(rating) = exif.rating.filter(|rating| *rating != 0) {
            attributes.push(format!("xmp:Rating=\"{rating}\""));
        }
    }
    if let Some(taken_at) = &reference.taken_at {
        attributes.push(format!("exif:DateTimeOriginal=\"{}\"", escaped(taken_at)));
    }
    let description = composed_description(reference);
    if attributes.is_empty() && description.is_empty() {
        return None;
    }
    let description_element = if description.is_empty() {
        String::new()
    } else {
        format!(
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>",
            escaped(&description)
        )
    };
    Some(format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:exif=\"http://ns.adobe.com/exif/1.0/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" {}>{}</rdf:Description></rdf:RDF></x:xmpmeta>",
        attributes.join(" "),
        description_element
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immich::refs::{Contributor, RefExif};

    fn reference(exif: Option<RefExif>, taken_at: Option<&str>, display_name: &str) -> AssetRef {
        AssetRef {
            origin_asset: "a1".into(),
            checksum: "sum".into(),
            checksum_alg: None,
            contributor: Contributor {
                display_name: display_name.into(),
                origin_user_id: None,
            },
            kind: "image".into(),
            taken_at: taken_at.map(str::to_string),
            exif,
        }
    }

    #[test]
    fn a_ref_with_nothing_to_say_has_no_sidecar() {
        assert_eq!(xmp_sidecar(&reference(None, None, "")), None);
    }

    #[test]
    fn a_place_is_written_as_degrees_and_decimal_minutes_with_a_hemisphere() {
        let exif = RefExif {
            latitude: Some(50.5),
            longitude: Some(-8.25),
            ..Default::default()
        };
        let xmp = xmp_sidecar(&reference(Some(exif), None, "")).unwrap();
        assert!(xmp.contains(r#"exif:GPSLatitude="50,30.000000N""#));
        assert!(xmp.contains(r#"exif:GPSLongitude="8,15.000000W""#));
    }

    #[test]
    fn a_place_needs_both_coordinates() {
        let exif = RefExif {
            latitude: Some(50.5),
            ..Default::default()
        };
        assert_eq!(xmp_sidecar(&reference(Some(exif), None, "")), None);
    }

    #[test]
    fn a_zero_rating_is_left_out_and_a_real_one_is_kept() {
        let none = RefExif {
            rating: Some(0),
            ..Default::default()
        };
        assert_eq!(xmp_sidecar(&reference(Some(none), None, "")), None);
        let four = RefExif {
            rating: Some(4),
            ..Default::default()
        };
        let xmp = xmp_sidecar(&reference(Some(four), None, "")).unwrap();
        assert!(xmp.contains(r#"xmp:Rating="4""#));
    }

    #[test]
    fn the_taken_date_is_written_as_the_original_date() {
        let xmp = xmp_sidecar(&reference(None, Some("2024-05-01T12:30:00.000Z"), "")).unwrap();
        assert!(xmp.contains(r#"exif:DateTimeOriginal="2024-05-01T12:30:00.000Z""#));
    }

    #[test]
    fn the_description_is_the_composed_one_and_is_escaped() {
        let exif = RefExif {
            description: Some("Tom & <Jerry> \"quoted\"".into()),
            ..Default::default()
        };
        let reference = reference(Some(exif), None, "Anna");
        let xmp = xmp_sidecar(&reference).unwrap();
        assert!(
            xmp.contains("Tom &amp; &lt;Jerry&gt; &quot;quoted&quot;\n\nShared by Anna</rdf:li>")
        );
        assert!(!composed_description(&reference).contains("&amp;"));
    }

    #[test]
    fn an_attribute_value_cannot_break_out_of_its_quotes() {
        let xmp = xmp_sidecar(&reference(None, Some("x\" evil=\"1"), "")).unwrap();
        assert!(!xmp.contains("evil=\"1\""));
    }
}

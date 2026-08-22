use std::path::Path;

pub fn qgis_style_filename(raster_filename: &str) -> String {
    let stem = Path::new(raster_filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("dtm_output");
    format!("{}_terrain.qlr", stem)
}

pub fn build_qgis_layer_definition(raster_filename: &str, id_suffix: &str) -> String {
    let raster_filename = xml_escape(raster_filename);
    let suffix: String = id_suffix
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(32)
        .collect();
    let suffix = if suffix.is_empty() { "dtm" } else { &suffix };
    let color_id = format!("dtm_color_{}", suffix);
    let hillshade_id = format!("dtm_hillshade_{}", suffix);
    let source = format!("./{}", raster_filename);

    format!(
        r##"<!DOCTYPE qgis-layer-definition>
<qlr>
  <layer-tree-group expanded="1" checked="Qt::Checked" name="DTM terrain">
    <customproperties/>
    <layer-tree-layer expanded="1" checked="Qt::Checked" id="{color_id}" name="Elevation colour (dynamic)" source="{source}" providerKey="gdal">
      <customproperties/>
    </layer-tree-layer>
    <layer-tree-layer expanded="1" checked="Qt::Checked" id="{hillshade_id}" name="Hillshade (dynamic)" source="{source}" providerKey="gdal">
      <customproperties/>
    </layer-tree-layer>
  </layer-tree-group>
  <maplayers>
    <maplayer type="raster" hasScaleBasedVisibilityFlag="0" minScale="100000000" maxScale="0" styleCategories="AllStyleCategories">
      <id>{color_id}</id>
      <datasource>{source}</datasource>
      <layername>Elevation colour (dynamic)</layername>
      <provider>gdal</provider>
      <customproperties>
        <property key="identify/format" value="Value"/>
      </customproperties>
      <pipe>
        <rasterrenderer type="singlebandpseudocolor" band="1" opacity="0.78" alphaBand="-1" classificationMin="0" classificationMax="1">
          <rasterTransparency/>
          <minMaxOrigin>
            <limits>CumulativeCut</limits>
            <extent>UpdatedCanvas</extent>
            <statAccuracy>Estimated</statAccuracy>
            <cumulativeCutLower>0.02</cumulativeCutLower>
            <cumulativeCutUpper>0.98</cumulativeCutUpper>
            <stdDevFactor>2</stdDevFactor>
          </minMaxOrigin>
          <rastershader>
            <colorrampshader colorRampType="INTERPOLATED" classificationMode="1" clip="0" minimumValue="0" maximumValue="1" labelPrecision="1">
              <colorramp type="gradient" name="[source]">
                <Option type="Map">
                  <Option name="color1" type="QString" value="43,131,186,255"/>
                  <Option name="color2" type="QString" value="255,255,255,255"/>
                  <Option name="discrete" type="bool" value="false"/>
                  <Option name="stops" type="QString" value="0.18;90,180,172,255:0.38;142,207,114,255:0.58;217,239,139,255:0.76;254,224,139,255:0.9;247,243,208,255"/>
                </Option>
              </colorramp>
              <item value="0" label="Low" color="#2b83ba" alpha="255"/>
              <item value="0.18" label="" color="#5ab4ac" alpha="255"/>
              <item value="0.38" label="" color="#8ecf72" alpha="255"/>
              <item value="0.58" label="" color="#d9ef8b" alpha="255"/>
              <item value="0.76" label="" color="#fee08b" alpha="255"/>
              <item value="0.9" label="" color="#f7f3d0" alpha="255"/>
              <item value="1" label="High" color="#ffffff" alpha="255"/>
            </colorrampshader>
          </rastershader>
        </rasterrenderer>
        <brightnesscontrast brightness="0" contrast="0" gamma="1"/>
        <huesaturation colorizeOn="0" grayscaleMode="0" saturation="0" colorizeRed="255" colorizeGreen="128" colorizeBlue="128" colorizeStrength="100"/>
        <rasterresampler maxOversampling="2"/>
      </pipe>
      <blendMode>13</blendMode>
    </maplayer>
    <maplayer type="raster" hasScaleBasedVisibilityFlag="0" minScale="100000000" maxScale="0" styleCategories="AllStyleCategories">
      <id>{hillshade_id}</id>
      <datasource>{source}</datasource>
      <layername>Hillshade (dynamic)</layername>
      <provider>gdal</provider>
      <customproperties>
        <property key="identify/format" value="Value"/>
      </customproperties>
      <pipe>
        <rasterrenderer type="hillshade" band="1" opacity="1" alphaBand="-1" azimuth="315" angle="45" zfactor="1" multidirection="1">
          <rasterTransparency/>
        </rasterrenderer>
        <brightnesscontrast brightness="4" contrast="12" gamma="1"/>
        <huesaturation colorizeOn="0" grayscaleMode="0" saturation="0" colorizeRed="255" colorizeGreen="128" colorizeBlue="128" colorizeStrength="100"/>
        <rasterresampler maxOversampling="2"/>
      </pipe>
      <blendMode>0</blendMode>
    </maplayer>
  </maplayers>
</qlr>
"##
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qgis_style_filename_uses_raster_stem() {
        assert_eq!(
            qgis_style_filename("dtm_output_12345678.tif"),
            "dtm_output_12345678_terrain.qlr"
        );
    }

    #[test]
    fn test_layer_definition_references_raster_twice_with_required_renderers() {
        let definition = build_qgis_layer_definition("dtm_output.tif", "job-123");

        assert_eq!(
            definition
                .matches("<datasource>./dtm_output.tif</datasource>")
                .count(),
            2
        );
        assert!(definition.contains("type=\"singlebandpseudocolor\""));
        assert!(definition.contains("<extent>UpdatedCanvas</extent>"));
        assert!(definition.contains("type=\"hillshade\""));
        assert!(definition.contains("<blendMode>13</blendMode>"));
        assert!(definition.contains("color=\"#2b83ba\""));
        assert!(definition.contains("color=\"#ffffff\""));
        assert!(
            definition.find("Elevation colour (dynamic)").unwrap()
                < definition.find("Hillshade (dynamic)").unwrap()
        );
    }

    #[test]
    fn test_layer_definition_escapes_filename_and_sanitizes_ids() {
        let definition = build_qgis_layer_definition("terrain & map.tif", "job-<123>");

        assert!(definition.contains("./terrain &amp; map.tif"));
        assert!(definition.contains("dtm_color_job123"));
        assert!(!definition.contains("job-<123>"));
    }
}

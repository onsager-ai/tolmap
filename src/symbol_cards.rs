//! Raster card geometry inside each displayed file footprint. The masks, not
//! the simplified output rings, are the ownership authority while recursing.
use std::collections::BTreeMap;

use anyhow::{ensure, Result};

use crate::blobs::marching_squares;
use crate::parcels::{point_in_polygon, polygon_area, rasterize, solve};
use crate::schema::{MapDocument, SymbolsDocument};

type Ring = Vec<[f64; 2]>;
// The prototype used roughly 9–12 vertices per card. A larger contour made
// dify's separate symbol document far heavier without adding visible detail.
const CARD_CONTOUR_POINTS: usize = 12;

struct Canvas {
    grid: usize,
    low: [f64; 2],
    step: f64,
}

fn ring(mask: &[bool], canvas: &Canvas) -> Option<Ring> {
    let mut rings = marching_squares(mask, canvas.grid);
    rings.sort_by(|a, b| {
        polygon_area(b)
            .total_cmp(&polygon_area(a))
            .then_with(|| a.len().cmp(&b.len()))
    });
    let points = rings.into_iter().next()?;
    let stride = points.len().div_ceil(CARD_CONTOUR_POINTS).max(1);
    let polygon = points
        .into_iter()
        .step_by(stride)
        .map(|p| {
            [
                ((canvas.low[0] + p[0] * canvas.step) * 1e9).round() / 1e9,
                ((canvas.low[1] + p[1] * canvas.step) * 1e9).round() / 1e9,
            ]
        })
        .collect::<Ring>();
    (polygon.len() >= 3 && polygon_area(&polygon) > 0.0).then_some(polygon)
}

fn inset(mask: &[bool], canvas: &Canvas, depth: usize, minimum_pixels: usize) -> Vec<bool> {
    let area = mask.iter().filter(|&&pixel| pixel).count() as f64 * canvas.step.powi(2);
    let distance = (if depth == 0 { 0.035 } else { 0.05 }) * area.sqrt();
    let cap = if depth == 0 { 0.004 } else { 0.003 };
    let radius = (distance.min(cap) / canvas.step).round() as usize;
    if radius == 0 {
        return mask.to_vec();
    }
    let g = canvas.grid;
    let mut result = mask.to_vec();
    for _ in 0..radius {
        let previous = result.clone();
        for (i, pixel) in result.iter_mut().enumerate() {
            if !previous[i] {
                continue;
            }
            let x = i % g;
            let y = i / g;
            *pixel = x > 0
                && x + 1 < g
                && y > 0
                && y + 1 < g
                && previous[i - 1]
                && previous[i + 1]
                && previous[i - g]
                && previous[i + g];
        }
    }
    // A thin but valid card keeps its one-cell sliver instead of vanishing.
    if result.iter().filter(|&&yes| yes).count() >= minimum_pixels {
        result
    } else {
        mask.to_vec()
    }
}

fn allocate(mask: &[bool], canvas: &Canvas, weights: &[f64]) -> Vec<Vec<bool>> {
    let count = weights.len();
    if count == 0 {
        return Vec::new();
    }
    let available = mask
        .iter()
        .enumerate()
        .filter_map(|(i, &yes)| yes.then_some(i))
        .collect::<Vec<_>>();
    if available.len() < count {
        return vec![vec![false; mask.len()]; count];
    }
    let sites = (0..count)
        .map(|i| {
            let cell = available[(i * available.len() / count).min(available.len() - 1)];
            [
                canvas.low[0] + (cell % canvas.grid) as f64 * canvas.step,
                canvas.low[1] + (cell / canvas.grid) as f64 * canvas.step,
            ]
        })
        .collect::<Vec<_>>();
    let owners = solve(
        mask,
        canvas.grid,
        canvas.low,
        canvas.step * (canvas.grid - 1) as f64,
        &sites,
        weights,
        1,
        true,
    );
    (0..count)
        .map(|owner| owners.iter().map(|&value| value == owner).collect())
        .collect()
}

fn reserve(mask: &mut [bool], canvas: &Canvas, parent: &Ring) -> Option<[f64; 4]> {
    let target = [
        parent.iter().map(|p| p[0]).sum::<f64>() / parent.len() as f64,
        parent.iter().map(|p| p[1]).sum::<f64>() / parent.len() as f64,
    ];
    let selected = mask
        .iter()
        .enumerate()
        .filter(|(_, yes)| **yes)
        .filter_map(|(i, _)| {
            let center = [
                canvas.low[0] + (i % canvas.grid) as f64 * canvas.step,
                canvas.low[1] + (i / canvas.grid) as f64 * canvas.step,
            ];
            point_in_polygon(center, parent).then_some((i, center))
        })
        .min_by(|a, b| {
            let da = (a.1[0] - target[0]).powi(2) + (a.1[1] - target[1]).powi(2);
            let db = (b.1[0] - target[0]).powi(2) + (b.1[1] - target[1]).powi(2);
            da.total_cmp(&db).then_with(|| a.0.cmp(&b.0))
        })?;
    mask[selected.0] = false;
    let mut half = canvas.step * 0.2;
    for _ in 0..24 {
        let corners = [
            [selected.1[0] - half, selected.1[1] - half],
            [selected.1[0] + half, selected.1[1] - half],
            [selected.1[0] + half, selected.1[1] + half],
            [selected.1[0] - half, selected.1[1] + half],
        ];
        if corners.iter().all(|&point| point_in_polygon(point, parent)) {
            break;
        }
        half *= 0.5;
    }
    Some([
        selected.1[0] - half,
        selected.1[1] - half,
        selected.1[0] + half,
        selected.1[1] + half,
    ])
}

fn valid_ring(ring: Option<&Ring>, parent: &Ring) -> bool {
    let Some(ring) = ring else { return false };
    let centroid = [
        ring.iter().map(|p| p[0]).sum::<f64>() / ring.len() as f64,
        ring.iter().map(|p| p[1]).sum::<f64>() / ring.len() as f64,
    ];
    let low = parent.iter().fold([f64::INFINITY; 2], |mut bounds, point| {
        bounds[0] = bounds[0].min(point[0]);
        bounds[1] = bounds[1].min(point[1]);
        bounds
    });
    let high = parent
        .iter()
        .fold([f64::NEG_INFINITY; 2], |mut bounds, point| {
            bounds[0] = bounds[0].max(point[0]);
            bounds[1] = bounds[1].max(point[1]);
            bounds
        });
    let epsilon = (high[0] - low[0]).max(high[1] - low[1]) * 1e-9;
    let near_edge = parent
        .iter()
        .zip(parent.iter().cycle().skip(1))
        .any(|(a, b)| {
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let length2 = dx * dx + dy * dy;
            let t = if length2 > 0.0 {
                ((centroid[0] - a[0]) * dx + (centroid[1] - a[1]) * dy) / length2
            } else {
                0.0
            }
            .clamp(0.0, 1.0);
            let ex = centroid[0] - (a[0] + t * dx);
            let ey = centroid[1] - (a[1] + t * dy);
            ex * ex + ey * ey <= epsilon * epsilon
        });
    point_in_polygon(centroid, parent) && !near_edge && polygon_area(ring) <= polygon_area(parent)
}

fn rectangle(rect: [f64; 4]) -> Ring {
    vec![
        [rect[0], rect[1]],
        [rect[2], rect[1]],
        [rect[2], rect[3]],
        [rect[0], rect[3]],
    ]
}

struct Cards<'a> {
    document: &'a SymbolsDocument,
    children: Vec<Vec<usize>>,
    rings: Vec<Option<Ring>>,
    headers: BTreeMap<usize, Ring>,
    modules: BTreeMap<usize, Ring>,
}

fn local_parent(document: &SymbolsDocument, symbol: usize) -> Option<usize> {
    let row = &document.symbols[symbol].0;
    (row.5 >= 0)
        .then_some(row.5 as usize)
        .filter(|&parent| document.symbols[parent].0 .0 == row.0)
}

impl Cards<'_> {
    fn own_lines(&self, symbol: usize) -> usize {
        let parent = &self.document.symbols[symbol].0;
        self.document.symbols[symbol].0 .6.saturating_sub(
            self.children[symbol]
                .iter()
                .filter(|&&child| {
                    let span = &self.document.symbols[child].0;
                    span.3 >= parent.3 && span.4 <= parent.4
                })
                .map(|&child| self.document.symbols[child].0 .6)
                .sum(),
        )
    }

    fn mass(&self, symbol: usize) -> usize {
        self.own_lines(symbol).max(1)
            + self.children[symbol]
                .iter()
                .map(|&child| self.mass(child))
                .sum::<usize>()
    }

    fn fallback(&mut self, symbol: usize, rect: [f64; 4]) {
        self.rings[symbol] = Some(rectangle(rect));
        let children = self.children[symbol].clone();
        if children.is_empty() {
            return;
        }
        let height = rect[3] - rect[1];
        let own = self.own_lines(symbol).max(1) as f64;
        let share = (own / self.mass(symbol) as f64).clamp(0.12, 0.4);
        let header_y = rect[3] - height * share;
        self.headers
            .insert(symbol, rectangle([rect[0], header_y, rect[2], rect[3]]));
        let sum = children
            .iter()
            .map(|&child| self.mass(child) as f64)
            .sum::<f64>();
        let mut x = rect[0];
        for child in children {
            let width = (rect[2] - rect[0]) * self.mass(child) as f64 / sum;
            let gap = width * 0.02;
            self.fallback(
                child,
                [
                    x + gap,
                    rect[1] + height * 0.02,
                    x + width - gap,
                    header_y - height * 0.02,
                ],
            );
            x += width;
        }
    }

    fn place(&mut self, symbol: usize, mask: &[bool], canvas: &Canvas, depth: usize) {
        let children = self.children[symbol].clone();
        let display = inset(mask, canvas, depth, children.len() * 2 + 1);
        self.rings[symbol] = ring(&display, canvas);
        if self.rings[symbol].is_none() {
            if let Some(pixel) = display.iter().position(|&yes| yes) {
                let center = [
                    canvas.low[0] + (pixel % canvas.grid) as f64 * canvas.step,
                    canvas.low[1] + (pixel / canvas.grid) as f64 * canvas.step,
                ];
                let half = canvas.step * 0.2;
                self.fallback(
                    symbol,
                    [
                        center[0] - half,
                        center[1] - half,
                        center[0] + half,
                        center[1] + half,
                    ],
                );
            }
            return;
        }
        if children.is_empty() {
            return;
        }
        let mut pixels = display
            .iter()
            .enumerate()
            .filter_map(|(i, &yes)| yes.then_some(i))
            .collect::<Vec<_>>();
        pixels.sort_by_key(|&i| (std::cmp::Reverse(i / canvas.grid), i % canvas.grid));
        let own = self.own_lines(symbol).max(1) as f64;
        let total = self.mass(symbol) as f64;
        let share = (own / total).clamp(0.12, 0.40);
        let header_count = ((pixels.len() as f64 * share).round() as usize)
            .max(1)
            .min(pixels.len().saturating_sub(children.len()));
        let mut header = vec![false; mask.len()];
        let mut body = display;
        for &pixel in pixels.iter().take(header_count) {
            header[pixel] = true;
            body[pixel] = false;
        }
        if let Some(outline) = ring(&header, canvas) {
            self.headers.insert(symbol, outline);
        }
        let reserve_rect = self.rings[symbol]
            .as_ref()
            .and_then(|parent| reserve(&mut body, canvas, parent));
        let header_valid = valid_ring(
            self.headers.get(&symbol),
            self.rings[symbol].as_ref().unwrap(),
        );
        if !header_valid {
            if let Some(rect) = reserve_rect {
                self.headers.insert(
                    symbol,
                    rectangle([
                        rect[0],
                        rect[1] + (rect[3] - rect[1]) * 0.8,
                        rect[2],
                        rect[3],
                    ]),
                );
            }
        }
        let weights = children
            .iter()
            .map(|&child| self.mass(child) as f64)
            .collect::<Vec<_>>();
        let regions = allocate(&body, canvas, &weights);
        for (&child, region) in children.iter().zip(&regions) {
            if region.contains(&true) {
                self.place(child, region, canvas, depth + 1);
            }
        }
        let parent = self.rings[symbol].as_ref().unwrap();
        let missing = children
            .iter()
            .copied()
            .filter(|&child| !valid_ring(self.rings[child].as_ref(), parent))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            if let Some(rect) = reserve_rect {
                let step = (rect[2] - rect[0]) / missing.len() as f64;
                let top = if !header_valid {
                    rect[1] + (rect[3] - rect[1]) * 0.78
                } else {
                    rect[3]
                };
                for (slot, child) in missing.iter().enumerate() {
                    let x = rect[0] + slot as f64 * step;
                    self.fallback(*child, [x + step * 0.02, rect[1], x + step * 0.98, top]);
                }
            }
        }
    }
}

pub fn attach(map: &MapDocument, document: &mut SymbolsDocument) -> Result<()> {
    let Some(parcels) = &map.parcels else {
        return Ok(());
    };
    let mut by_file = vec![Vec::new(); map.files.len()];
    let mut children = vec![Vec::new(); document.symbols.len()];
    for (i, row) in document.symbols.iter().enumerate() {
        by_file[row.0 .0].push(i);
        if let Some(parent) = local_parent(document, i) {
            children[parent].push(i);
        }
    }
    let mut cards = Cards {
        document,
        children,
        rings: vec![None; document.symbols.len()],
        headers: BTreeMap::new(),
        modules: BTreeMap::new(),
    };
    for (file, symbols) in by_file.iter().enumerate() {
        let Some(polygon) = parcels.get(&file.to_string()) else {
            continue;
        };
        let top = symbols
            .iter()
            .copied()
            .filter(|&symbol| local_parent(document, symbol).is_none())
            .collect::<Vec<_>>();
        let module_lines = document.module_code_lines.get(&file).copied().unwrap_or(0);
        if top.is_empty() && module_lines == 0 {
            continue;
        }
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for point in polygon {
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
        let span = (high[0] - low[0]).max(high[1] - low[1]).max(1e-9);
        let padding = span * 0.01;
        low = [low[0] - padding, low[1] - padding];
        let span = span + 2.0 * padding;
        let mut grid = ((symbols.len() + usize::from(module_lines > 0)) as f64 * 40.0)
            .sqrt()
            .ceil() as usize;
        grid = grid.clamp(32, 320);
        let has_module = module_lines > 0;
        let weights = (if has_module {
            vec![module_lines as f64]
        } else {
            vec![]
        })
        .into_iter()
        .chain(top.iter().map(|&i| cards.mass(i) as f64))
        .collect::<Vec<_>>();
        loop {
            let mut mask = rasterize(std::slice::from_ref(polygon), low, span, grid);
            let canvas = Canvas {
                grid,
                low,
                step: span / (grid - 1) as f64,
            };
            for &symbol in symbols {
                cards.rings[symbol] = None;
                cards.headers.remove(&symbol);
            }
            cards.modules.remove(&file);
            if mask.iter().filter(|&&yes| yes).count()
                >= symbols.len() * 12 + usize::from(has_module)
                || (grid == 1024 && mask.contains(&true))
            {
                let reserve_rect = reserve(&mut mask, &canvas, polygon);
                let regions = allocate(&mask, &canvas, &weights);
                let mut offset = 0;
                if has_module {
                    if let Some(outline) = ring(&inset(&regions[0], &canvas, 0, 1), &canvas) {
                        cards.modules.insert(file, outline);
                    }
                    offset = 1;
                }
                for (&symbol, region) in top.iter().zip(regions.iter().skip(offset)) {
                    if region.contains(&true) {
                        cards.place(symbol, region, &canvas, 0);
                    }
                }
                let mut missing = Vec::new();
                if has_module && !valid_ring(cards.modules.get(&file), polygon) {
                    missing.push(None);
                }
                missing.extend(
                    top.iter()
                        .copied()
                        .filter(|&i| !valid_ring(cards.rings[i].as_ref(), polygon))
                        .map(Some),
                );
                if let Some(rect) = reserve_rect {
                    let step = (rect[2] - rect[0]) / missing.len().max(1) as f64;
                    for (slot, entry) in missing.into_iter().enumerate() {
                        let x = rect[0] + slot as f64 * step;
                        let sub = [x + step * 0.02, rect[1], x + step * 0.98, rect[3]];
                        if let Some(symbol) = entry {
                            cards.fallback(symbol, sub);
                        } else {
                            cards.modules.insert(file, rectangle(sub));
                        }
                    }
                }
            }
            let complete = symbols.iter().all(|&i| {
                document.symbols[i].0 .6 == 0 || valid_ring(cards.rings[i].as_ref(), polygon)
            }) && (!has_module || valid_ring(cards.modules.get(&file), polygon));
            if complete {
                break;
            }
            ensure!(
                grid < 1024,
                "missing card in file {file} at maximum raster resolution"
            );
            grid = (grid * 2).min(1024);
        }
    }
    for (i, row) in document.symbols.iter().enumerate() {
        if row.0 .6 >= 1 && parcels.contains_key(&row.0 .0.to_string()) {
            ensure!(cards.rings[i].is_some(), "missing card for symbol {i}");
        }
    }
    let Cards {
        rings,
        modules,
        headers,
        ..
    } = cards;
    document.symbol_rings = Some(rings);
    document.module_rings = Some(modules);
    document.header_rings = Some(headers);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parcels::point_in_polygon;

    fn square_canvas(grid: usize) -> (Canvas, Vec<bool>) {
        (
            Canvas {
                grid,
                low: [0.0, 0.0],
                step: 1.0 / (grid - 1) as f64,
            },
            vec![true; grid * grid],
        )
    }

    #[test]
    fn one_and_two_hundred_symbols_keep_a_connected_nonoverlapping_region() {
        for count in [1, 200] {
            let (canvas, mask) = square_canvas(100);
            let regions = allocate(&mask, &canvas, &vec![1.0; count]);
            let mut claimed = vec![false; mask.len()];
            for region in &regions {
                assert!(region.contains(&true));
                let outline = ring(region, &canvas).expect("every owner has an outline");
                assert!(polygon_area(&outline) > 0.0);
                for (i, &yes) in region.iter().enumerate() {
                    if yes {
                        assert!(!claimed[i], "sibling masks overlap");
                        claimed[i] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn child_card_centroid_and_area_stay_inside_parent() {
        let (canvas, mask) = square_canvas(80);
        let parent = ring(&inset(&mask, &canvas, 0, 1), &canvas).unwrap();
        let body = inset(&mask, &canvas, 0, 1);
        for child in allocate(&body, &canvas, &[1.0, 3.0, 2.0]) {
            let child = ring(&inset(&child, &canvas, 1, 1), &canvas).unwrap();
            let center = [
                child.iter().map(|p| p[0]).sum::<f64>() / child.len() as f64,
                child.iter().map(|p| p[1]).sum::<f64>() / child.len() as f64,
            ];
            assert!(point_in_polygon(center, &parent));
            assert!(polygon_area(&child) < polygon_area(&parent));
        }
    }

    #[test]
    fn card_areas_follow_unequal_code_lines() {
        let (canvas, mask) = square_canvas(80);
        let regions = allocate(&mask, &canvas, &[1.0, 2.0, 4.0]);
        let areas = regions
            .iter()
            .map(|region| region.iter().filter(|&&yes| yes).count())
            .collect::<Vec<_>>();
        assert!(areas[0] < areas[1] && areas[1] < areas[2], "{areas:?}");
    }

    #[test]
    fn type_mass_includes_methods_outside_its_source_span() {
        let document: SymbolsDocument = serde_json::from_value(serde_json::json!({
            "files": [0],
            "symbols": [[0,"T",0,1,2,-1,2],[0,"A",2,10,12,0,3],[0,"B",2,14,16,0,3]],
            "edges": [],
            "module_code_lines": {"0": 0},
            "coverage": {"calls_total":0,"calls_resolved":0,"unresolved":{}}
        }))
        .unwrap();
        let cards = Cards {
            rings: vec![None; 3],
            children: vec![vec![1, 2], vec![], vec![]],
            headers: BTreeMap::new(),
            modules: BTreeMap::new(),
            document: &document,
        };
        assert_eq!(cards.mass(0), 8);
    }

    #[test]
    fn one_pixel_parent_still_gives_two_hundred_children_distinct_rings() {
        use crate::schema::{HierSymbolRow, SymbolCoverage};
        let mut symbols = vec![HierSymbolRow((0, "Parent".into(), 0, 1, 201, -1, 201))];
        for i in 0..200 {
            symbols.push(HierSymbolRow((
                0,
                format!("child{i}"),
                2,
                i + 2,
                i + 2,
                0,
                1,
            )));
        }
        let document = SymbolsDocument {
            files: vec![0],
            symbols,
            edges: Vec::new(),
            module_code_lines: BTreeMap::new(),
            coverage: SymbolCoverage::default(),
            symbol_rings: None,
            module_rings: None,
            header_rings: None,
        };
        let (canvas, _) = square_canvas(8);
        let mut mask = vec![false; 64];
        mask[27] = true;
        let mut cards = Cards {
            rings: vec![None; 201],
            children: std::iter::once((1..201).collect())
                .chain(std::iter::repeat_with(Vec::new).take(200))
                .collect(),
            headers: BTreeMap::new(),
            modules: BTreeMap::new(),
            document: &document,
        };
        cards.place(0, &mask, &canvas, 0);
        let parent = cards.rings[0].as_ref().unwrap();
        let mut previous_right = f64::NEG_INFINITY;
        for child in cards.rings.iter().skip(1) {
            let child = child.as_ref().unwrap();
            assert!(point_in_polygon(child[0], parent));
            assert!(child[0][0] > previous_right);
            previous_right = child[1][0];
        }
    }

    #[test]
    fn go_method_with_receiver_in_another_file_gets_local_card() {
        let document: SymbolsDocument = serde_json::from_value(serde_json::json!({
            "files": [0, 1],
            "symbols": [[0,"Receiver",0,1,2,-1,2],[1,"Method",2,10,12,0,3]],
            "edges": [],
            "module_code_lines": {"0": 0, "1": 0},
            "coverage": {"calls_total":0,"calls_resolved":0,"unresolved":{}}
        }))
        .unwrap();
        assert_eq!(local_parent(&document, 0), None);
        assert_eq!(local_parent(&document, 1), None);
        assert_eq!(document.symbols[1].0 .5, 0);
    }

    #[test]
    fn boundary_centroid_is_not_accepted_as_contained() {
        let parent = rectangle([0.0, 0.0, 1.0, 1.0]);
        let touching = rectangle([0.9, 0.4, 1.1, 0.6]);
        assert!(!valid_ring(Some(&touching), &parent));
        let interior = rectangle([0.7, 0.4, 0.9, 0.6]);
        assert!(valid_ring(Some(&interior), &parent));
    }
}

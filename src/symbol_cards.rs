//! Raster card geometry inside each displayed file footprint. The masks, not
//! the simplified output rings, are the ownership authority while recursing.
use std::collections::BTreeMap;

use anyhow::{ensure, Result};

use crate::blobs::marching_squares;
use crate::parcels::{polygon_area, rasterize, solve};
use crate::schema::{MapDocument, SymbolsDocument};

type Ring = Vec<[f64; 2]>;

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
    let stride = points.len().div_ceil(64).max(1);
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

fn inset(mask: &[bool], canvas: &Canvas, depth: usize) -> Vec<bool> {
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
    if result.contains(&true) {
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

struct Cards<'a> {
    document: &'a SymbolsDocument,
    children: Vec<Vec<usize>>,
    rings: Vec<Option<Ring>>,
    headers: BTreeMap<usize, Ring>,
    modules: BTreeMap<usize, Ring>,
}

impl Cards<'_> {
    fn own_lines(&self, symbol: usize) -> usize {
        self.document.symbols[symbol].0 .6.saturating_sub(
            self.children[symbol]
                .iter()
                .map(|&child| self.document.symbols[child].0 .6)
                .sum(),
        )
    }

    fn place(&mut self, symbol: usize, mask: &[bool], canvas: &Canvas, depth: usize) {
        let display = inset(mask, canvas, depth);
        self.rings[symbol] = ring(&display, canvas);
        let children = self.children[symbol].clone();
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
        let total = self.document.symbols[symbol].0 .6.max(1) as f64;
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
        let weights = children
            .iter()
            .map(|&child| self.document.symbols[child].0 .6.max(1) as f64)
            .collect::<Vec<_>>();
        let regions = allocate(&body, canvas, &weights);
        for (&child, region) in children.iter().zip(regions) {
            self.place(child, &region, canvas, depth + 1);
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
        if row.0 .5 >= 0 {
            children[row.0 .5 as usize].push(i);
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
            .filter(|&symbol| document.symbols[symbol].0 .5 < 0)
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
        let mut mask = rasterize(std::slice::from_ref(polygon), low, span, grid);
        while mask.iter().filter(|&&yes| yes).count() < symbols.len() * 8 + 8 && grid < 1024 {
            grid = (grid * 2).min(1024);
            mask = rasterize(std::slice::from_ref(polygon), low, span, grid);
        }
        let canvas = Canvas {
            grid,
            low,
            step: span / (grid - 1) as f64,
        };
        let has_module = module_lines > 0;
        let weights = (if has_module {
            vec![module_lines as f64]
        } else {
            vec![]
        })
        .into_iter()
        .chain(top.iter().map(|&i| document.symbols[i].0 .6.max(1) as f64))
        .collect::<Vec<_>>();
        let regions = allocate(&mask, &canvas, &weights);
        let mut offset = 0;
        if has_module {
            if let Some(outline) = ring(&inset(&regions[0], &canvas, 0), &canvas) {
                cards.modules.insert(file, outline);
            }
            offset = 1;
        }
        for (&symbol, region) in top.iter().zip(regions.iter().skip(offset)) {
            cards.place(symbol, region, &canvas, 0);
        }
    }
    for (i, row) in document.symbols.iter().enumerate() {
        if row.0 .6 >= 1 && parcels.contains_key(&row.0 .0.to_string()) {
            ensure!(cards.rings[i].is_some(), "missing card for symbol {i}");
        }
    }
    document.symbol_rings = Some(cards.rings);
    document.module_rings = Some(cards.modules);
    document.header_rings = Some(cards.headers);
    Ok(())
}

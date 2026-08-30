// Classic squarified treemap layout (Bruls, Huizing, van Wijk). Operates in
// a normalized 0..100 x 0..100 coordinate space so callers can render with
// plain CSS percentages regardless of the actual pixel container size.

export interface SquarifyInput {
  id: string;
  size: number;
}

export interface SquarifyRect extends SquarifyInput {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Sized extends SquarifyInput {
  area: number;
}

function worst(row: Sized[], length: number): number {
  if (row.length === 0) return Infinity;
  const sum = row.reduce((s, r) => s + r.area, 0);
  const max = Math.max(...row.map((r) => r.area));
  const min = Math.min(...row.map((r) => r.area));
  if (sum === 0 || min === 0) return Infinity;
  return Math.max((length * length * max) / (sum * sum), (sum * sum) / (length * length * min));
}

export function squarify(items: SquarifyInput[], width = 100, height = 100): SquarifyRect[] {
  const positive = items.filter((i) => i.size > 0);
  const total = positive.reduce((s, i) => s + i.size, 0);
  if (total <= 0 || positive.length === 0) return [];

  const scale = (width * height) / total;
  const sorted: Sized[] = [...positive].sort((a, b) => b.size - a.size).map((i) => ({ ...i, area: i.size * scale }));

  const rects: SquarifyRect[] = [];
  let remaining = sorted;
  let rx = 0;
  let ry = 0;
  let rw = width;
  let rh = height;

  while (remaining.length > 0) {
    const length = Math.min(rw, rh);
    let row: Sized[] = [remaining[0]];
    let i = 1;
    while (i < remaining.length) {
      const candidate = [...row, remaining[i]];
      if (worst(candidate, length) > worst(row, length)) break;
      row = candidate;
      i += 1;
    }

    const rowArea = row.reduce((s, r) => s + r.area, 0);
    const rowLength = length > 0 ? rowArea / length : 0;

    if (rw >= rh) {
      let oy = ry;
      for (const item of row) {
        const itemHeight = rowLength > 0 ? item.area / rowLength : 0;
        rects.push({ ...item, x: rx, y: oy, width: rowLength, height: itemHeight });
        oy += itemHeight;
      }
      rx += rowLength;
      rw -= rowLength;
    } else {
      let ox = rx;
      for (const item of row) {
        const itemWidth = rowLength > 0 ? item.area / rowLength : 0;
        rects.push({ ...item, x: ox, y: ry, width: itemWidth, height: rowLength });
        ox += itemWidth;
      }
      ry += rowLength;
      rh -= rowLength;
    }

    remaining = remaining.slice(row.length);
  }

  return rects;
}

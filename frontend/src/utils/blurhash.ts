const DIGITS =
  "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";

const dataUrlCache = new Map<string, string | null>();

export function blurHashToDataUrl(
  blurhash: string,
  width = 32,
  height = 32,
): string | null {
  const cacheKey = `${blurhash}:${width}x${height}`;
  if (dataUrlCache.has(cacheKey)) return dataUrlCache.get(cacheKey) ?? null;

  try {
    const pixels = decodeBlurHash(blurhash, width, height);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) {
      dataUrlCache.set(cacheKey, null);
      return null;
    }
    const imageData = context.createImageData(width, height);
    imageData.data.set(pixels);
    context.putImageData(imageData, 0, 0);
    const dataUrl = canvas.toDataURL("image/png");
    dataUrlCache.set(cacheKey, dataUrl);
    return dataUrl;
  } catch {
    dataUrlCache.set(cacheKey, null);
    return null;
  }
}

function decodeBlurHash(
  blurhash: string,
  width: number,
  height: number,
): Uint8ClampedArray {
  if (blurhash.length < 6) throw new Error("Invalid blurhash");

  const sizeFlag = decode83(blurhash[0]);
  const numY = Math.floor(sizeFlag / 9) + 1;
  const numX = (sizeFlag % 9) + 1;
  const quantisedMaximumValue = decode83(blurhash[1]);
  const maximumValue = (quantisedMaximumValue + 1) / 166;
  const expectedLength = 4 + 2 * numX * numY;
  if (blurhash.length !== expectedLength) throw new Error("Invalid blurhash");

  const colors = new Array<[number, number, number]>(numX * numY);
  for (let index = 0; index < colors.length; index += 1) {
    if (index === 0) {
      colors[index] = decodeDc(decode83(blurhash.slice(2, 6)));
    } else {
      colors[index] = decodeAc(
        decode83(blurhash.slice(4 + index * 2, 6 + index * 2)),
        maximumValue,
      );
    }
  }

  const bytesPerRow = width * 4;
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      let r = 0;
      let g = 0;
      let b = 0;

      for (let j = 0; j < numY; j += 1) {
        for (let i = 0; i < numX; i += 1) {
          const basis =
            Math.cos((Math.PI * x * i) / width) *
            Math.cos((Math.PI * y * j) / height);
          const color = colors[i + j * numX];
          r += color[0] * basis;
          g += color[1] * basis;
          b += color[2] * basis;
        }
      }

      const pixelOffset = y * bytesPerRow + x * 4;
      pixels[pixelOffset] = linearToSrgb(r);
      pixels[pixelOffset + 1] = linearToSrgb(g);
      pixels[pixelOffset + 2] = linearToSrgb(b);
      pixels[pixelOffset + 3] = 255;
    }
  }

  return pixels;
}

function decode83(value: string): number {
  let result = 0;
  for (const character of value) {
    const digit = DIGITS.indexOf(character);
    if (digit === -1) throw new Error("Invalid blurhash");
    result = result * 83 + digit;
  }
  return result;
}

function decodeDc(value: number): [number, number, number] {
  const r = value >> 16;
  const g = (value >> 8) & 255;
  const b = value & 255;
  return [srgbToLinear(r), srgbToLinear(g), srgbToLinear(b)];
}

function decodeAc(value: number, maximumValue: number): [number, number, number] {
  const quantR = Math.floor(value / (19 * 19));
  const quantG = Math.floor(value / 19) % 19;
  const quantB = value % 19;
  return [
    signPow((quantR - 9) / 9, 2) * maximumValue,
    signPow((quantG - 9) / 9, 2) * maximumValue,
    signPow((quantB - 9) / 9, 2) * maximumValue,
  ];
}

function signPow(value: number, exponent: number) {
  return Math.sign(value) * Math.pow(Math.abs(value), exponent);
}

function srgbToLinear(value: number) {
  const normalized = value / 255;
  if (normalized <= 0.04045) return normalized / 12.92;
  return Math.pow((normalized + 0.055) / 1.055, 2.4);
}

function linearToSrgb(value: number) {
  const clamped = Math.max(0, Math.min(1, value));
  if (clamped <= 0.0031308) return Math.round(clamped * 12.92 * 255);
  return Math.round((1.055 * Math.pow(clamped, 1 / 2.4) - 0.055) * 255);
}

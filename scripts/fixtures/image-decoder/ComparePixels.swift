import Foundation
import ImageIO
import CoreGraphics
let paths = Array(CommandLine.arguments.dropFirst())
func pixels(_ path: String) -> (Int, Int, [UInt8]) {
    let source = CGImageSourceCreateWithURL(URL(fileURLWithPath:path) as CFURL, nil)!
    let image = CGImageSourceCreateImageAtIndex(source,0,nil)!
    let width=image.width, height=image.height
    var data=[UInt8](repeating:0,count:width*height*4)
    data.withUnsafeMutableBytes { bytes in
        let ctx=CGContext(data:bytes.baseAddress,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.draw(image,in:CGRect(x:0,y:0,width:width,height:height))
    }
    return (width,height,data)
}
let a=pixels(paths[0]), b=pixels(paths[1])
precondition(a.0==b.0 && a.1==b.1)
var changed=0
for i in stride(from:0,to:a.2.count,by:4) { if a.2[i..<i+4] != b.2[i..<i+4] { changed += 1 } }
print("{\"width\":\(a.0),\"height\":\(a.1),\"changedPixels\":\(changed),\"totalPixels\":\(a.0*a.1)}")

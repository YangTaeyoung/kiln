// Existing Kiln geometry only. No generated provider/brand marks.
import sharp from 'sharp';
import {readFile,writeFile,readdir} from 'node:fs/promises';
const source=await readFile(new URL('../public/assets/kiln.svg',import.meta.url));
async function icon(size,mark=size*.8){return sharp({create:{width:size,height:size,channels:4,background:'#f7f6f2'}}).composite([{input:await sharp(source).resize(Math.round(mark),Math.round(mark)).png().toBuffer(),gravity:'centre'}]).png().toBuffer();}
await writeFile('ios/App/App/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png',await icon(1024));
for(const [density,size] of Object.entries({mdpi:48,hdpi:72,xhdpi:96,xxhdpi:144,xxxhdpi:192})){
 for(const name of ['ic_launcher','ic_launcher_round'])await writeFile(`android/app/src/main/res/mipmap-${density}/${name}.png`,await icon(size));
 await writeFile(`android/app/src/main/res/mipmap-${density}/ic_launcher_foreground.png`,await icon(Math.round(size*2.25),size*1.25));
}
for(const name of ['splash-2732x2732-2.png','splash-2732x2732-1.png','splash-2732x2732.png'])await writeFile(`ios/App/App/Assets.xcassets/Splash.imageset/${name}`,await icon(2732,480));
for(const dir of await readdir('android/app/src/main/res'))if(dir.startsWith('drawable')){
 try{await readFile(`android/app/src/main/res/${dir}/splash.png`);await writeFile(`android/app/src/main/res/${dir}/splash.png`,await icon(768,150));}catch{}
}

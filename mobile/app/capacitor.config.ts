import type { CapacitorConfig } from '@capacitor/cli';
const config: CapacitorConfig = {
 appId:'dev.kiln.remote', appName:'Kiln Remote', webDir:'dist', loggingBehavior:'none',
 backgroundColor:'#f7f6f2',
 ios:{contentInset:'never'}, plugins:{SystemBars:{insetsHandling:'css',initialViewportFitValueHint:'cover'}},
};
export default config;

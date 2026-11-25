export const matrix={include:[
  {runner:'windows-2025',target:'x86_64-pc-windows-msvc',bundles:'nsis'},
  {runner:'windows-11-arm',target:'aarch64-pc-windows-msvc',bundles:'nsis'},
  {runner:'ubuntu-22.04',target:'x86_64-unknown-linux-gnu',bundles:'deb,appimage'},
  {runner:'ubuntu-22.04-arm',target:'aarch64-unknown-linux-gnu',bundles:'deb,appimage'},
  {runner:'macos-15',target:'aarch64-apple-darwin',bundles:'dmg,app'},
]};
export const targets=matrix.include.map(item=>item.target);
if(process.argv[1]?.replaceAll('\\','/').endsWith('/targets.mjs'))console.log(JSON.stringify(matrix));

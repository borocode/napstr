import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const main = resolve('src-tauri/gen/android/app/src/main');
const manifestPath = resolve(main, 'AndroidManifest.xml');
let manifest = await readFile(manifestPath, 'utf8');

const application = '<application';
if (manifest.includes('android:allowBackup=')) {
  manifest = manifest.replace(/android:allowBackup="[^"]*"/, 'android:allowBackup="false"');
} else {
  manifest = manifest.replace(application, `${application}\n        android:allowBackup="false"`);
}
if (!manifest.includes('android:dataExtractionRules=')) {
  manifest = manifest.replace(
    application,
    `${application}\n        android:dataExtractionRules="@xml/data_extraction_rules"`,
  );
}
if (!manifest.includes('android:fullBackupContent=')) {
  manifest = manifest.replace(
    application,
    `${application}\n        android:fullBackupContent="@xml/backup_rules"`,
  );
}
await writeFile(manifestPath, manifest);

const xmlDirectory = resolve(main, 'res/xml');
await mkdir(xmlDirectory, { recursive: true });
await writeFile(
  resolve(xmlDirectory, 'backup_rules.xml'),
  `<?xml version="1.0" encoding="utf-8"?>
<full-backup-content>
    <exclude domain="root" path="." />
    <exclude domain="file" path="." />
    <exclude domain="database" path="." />
    <exclude domain="sharedpref" path="." />
    <exclude domain="external" path="." />
</full-backup-content>
  `,
);

if (process.env.NAPSTRFY_ANDROID_RELEASE_SIGNING === '1') {
  const gradlePath = resolve('src-tauri/gen/android/app/build.gradle.kts');
  let gradle = await readFile(gradlePath, 'utf8');

  if (!gradle.includes('import java.io.FileInputStream')) {
    gradle = gradle.replace(
      'import java.util.Properties',
      'import java.io.FileInputStream\nimport java.util.Properties',
    );
  }

  if (!gradle.includes('create("release")')) {
    gradle = gradle.replace(
      '    buildTypes {',
      `    signingConfigs {
        create("release") {
            val keystorePropertiesFile = rootProject.file("keystore.properties")
            require(keystorePropertiesFile.exists()) {
                "Release signing requires gen/android/keystore.properties"
            }
            val keystoreProperties = Properties().apply {
                load(FileInputStream(keystorePropertiesFile))
            }
            keyAlias = keystoreProperties["keyAlias"] as String
            keyPassword = keystoreProperties["password"] as String
            storeFile = file(keystoreProperties["storeFile"] as String)
            storePassword = keystoreProperties["password"] as String
        }
    }
    buildTypes {`,
    );
  }

  if (!gradle.includes('signingConfig = signingConfigs.getByName("release")')) {
    gradle = gradle.replace(
      '        getByName("release") {',
      '        getByName("release") {\n            signingConfig = signingConfigs.getByName("release")',
    );
  }

  await writeFile(gradlePath, gradle);
}
await writeFile(
  resolve(xmlDirectory, 'data_extraction_rules.xml'),
  `<?xml version="1.0" encoding="utf-8"?>
<data-extraction-rules>
    <cloud-backup>
        <exclude domain="root" path="." />
        <exclude domain="file" path="." />
        <exclude domain="database" path="." />
        <exclude domain="sharedpref" path="." />
        <exclude domain="external" path="." />
    </cloud-backup>
    <device-transfer>
        <exclude domain="root" path="." />
        <exclude domain="file" path="." />
        <exclude domain="database" path="." />
        <exclude domain="sharedpref" path="." />
        <exclude domain="external" path="." />
    </device-transfer>
</data-extraction-rules>
`,
);

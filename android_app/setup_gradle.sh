#!/bin/bash
set -e

# Configuration
GRADLE_VERSION="8.5"
GRADLE_ZIP="gradle-${GRADLE_VERSION}-bin.zip"
GRADLE_URL="https://services.gradle.org/distributions/${GRADLE_ZIP}"

# JDK 17 Configuration (System Java 25 is too new for Gradle)
JDK_VERSION="17.0.10"
JDK_BUILD="7"
JDK_URL="https://github.com/adoptium/temurin17-binaries/releases/download/jdk-${JDK_VERSION}%2B${JDK_BUILD}/OpenJDK17U-jdk_x64_linux_hotspot_${JDK_VERSION}_${JDK_BUILD}.tar.gz"
LOCAL_JDK_DIR="local_jdk"

# Move to the script's directory (android_app root)
cd "$(dirname "$0")"
PROJECT_ROOT="$(pwd)"

echo "=== Setting up Environment for Rignite Android App ==="

# --- Step 1: Setup Local JDK 17 ---

if [ ! -d "${LOCAL_JDK_DIR}" ]; then
    echo "System Java might be too new. Downloading JDK 17..."
    if command -v wget >/dev/null 2>&1; then
        wget -q -O jdk.tar.gz "${JDK_URL}"
    else
        echo "Error: wget is required but not found."
        exit 1
    fi

    echo "Extracting JDK..."
    mkdir -p "${LOCAL_JDK_DIR}"
    tar -xzf jdk.tar.gz -C "${LOCAL_JDK_DIR}" --strip-components=1
    rm jdk.tar.gz
    echo "JDK 17 installed to ${LOCAL_JDK_DIR}"
fi

# Set Java Environment for this script
export JAVA_HOME="${PROJECT_ROOT}/${LOCAL_JDK_DIR}"
export PATH="${JAVA_HOME}/bin:${PATH}"

echo "Using Java: $(java -version 2>&1 | head -n 1)"

# --- Step 2: Setup Gradle Wrapper ---

# Check if gradlew already exists
if [ -f "./gradlew" ]; then
    echo "Gradle Wrapper already exists."
else
    # Download Gradle
    if [ ! -d "gradle-${GRADLE_VERSION}" ]; then
        echo "Downloading Gradle ${GRADLE_VERSION}..."
        if command -v wget >/dev/null 2>&1; then
            wget -q "${GRADLE_URL}"
        else
            echo "Error: wget is required but not found."
            exit 1
        fi

        echo "Unzipping Gradle..."
        unzip -q "${GRADLE_ZIP}"
    fi

    # Generate Wrapper
    echo "Generating Gradle Wrapper..."
    "./gradle-${GRADLE_VERSION}/bin/gradle" wrapper --gradle-version "${GRADLE_VERSION}"

    # Cleanup Gradle Dist
    echo "Cleaning up temporary Gradle files..."
    rm "${GRADLE_ZIP}"
    rm -rf "gradle-${GRADLE_VERSION}"

    # Make executable
    chmod +x gradlew
fi

# --- Step 3: Create Build Helper ---

echo "Creating 'build_app.sh' helper script..."
cat > build_app.sh <<EOF
#!/bin/bash
export JAVA_HOME="${PROJECT_ROOT}/${LOCAL_JDK_DIR}"
export PATH="\${JAVA_HOME}/bin:\${PATH}"
./gradlew assembleDebug
EOF
chmod +x build_app.sh

echo "=== Success! Setup complete. ==="
echo "To build the app, run: ./build_app.sh"

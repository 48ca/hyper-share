#!/bin/bash -ue

file="$1"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

CR=$(echo -ne '\r')

# Not sure how to rename the file upon upload with curl, so
# going to upload it to a subdirectory.
mkdir -p $DIR/curl-upload
output_file="$file.out"

pushd $DIR > /dev/null

output_up=$(curl --form "fileupload=@$file" http://localhost:$PORT/curl-upload)
output_down=$(wget -O "$output_file" "http://localhost:$PORT/curl-upload/$file")

popd > /dev/null

# echo "Comparing files"

res="$(md5sum "$DIR/$file" "$DIR/$output_file" | awk '{ print $1 }')"

res1=$(echo $res | awk '{ print $1 }')
res2=$(echo $res | awk '{ print $2 }')

if [[ "$res1" ==  "$res2" ]]
then
    echo -e "${GREEN}Passed${NC}"
else
    echo -e "${RED}Failed!!!${NC}"
    echo "Source: $res1"
    echo "Output: $res2"

    echo "curl up output: $output_up"
    echo "curl down output: $output_down"
fi

rm "$DIR/$output_file"
